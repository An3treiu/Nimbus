use axum::{
    extract::{Path, Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use nimbus_ai::ChatProvider;
use nimbus_core::DriveFile;
use nimbus_crypto::Vault;
use nimbus_github::{poll_for_token, start_device_flow, DeviceCode, GitHubClient, PollResult};
use nimbus_search::{SearchHit, SearchIndex};
use nimbus_storage::StorageEngine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::sync::Arc;

/// GitHub's OAuth host (device-flow endpoints live here, not on api.github.com).
const GITHUB_OAUTH_BASE: &str = "https://github.com";
/// GitHub REST API root, used to verify the authenticated user.
const GITHUB_API_BASE: &str = "https://api.github.com";

/// Shared application state: the storage engine plus optional AI features.
#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<StorageEngine>,
    pub search: Option<Arc<SearchIndex>>,
    pub chat: Option<Arc<dyn ChatProvider>>,
    /// Pool for persisting the OAuth token.
    pub pool: SqlitePool,
    /// OAuth App client id; `None` disables the device-flow endpoints.
    pub github_client_id: Option<String>,
    /// The account expected to own the drive — OAuth tokens are verified against it.
    pub drive_owner: String,
    /// Vault for encrypting the persisted OAuth token at rest (if encryption is on).
    pub vault: Option<Vault>,
    /// When set, `/api/*` requires this token (Bearer header or cookie).
    pub admin_token: Option<String>,
}

/// Auth gate for `/api/*`. When `admin_token` is configured, requires a matching
/// `Authorization: Bearer` header or `nimbus_token` cookie; otherwise open
/// (intended for loopback-only dev — the server refuses to bind publicly
/// without a token configured).
fn cookie_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies
                .split(';')
                .filter_map(|c| c.trim().strip_prefix(prefix.as_str()))
                .next()
                .map(str::to_string)
        })
}

async fn require_auth(
    State(st): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // 1. Admin token (Bearer header or nimbus_token cookie).
    if let Some(expected) = &st.admin_token {
        let from_header = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map(str::to_string);
        let from_cookie = cookie_value(req.headers(), "nimbus_token");
        if let Some(provided) = from_header.or(from_cookie) {
            if nimbus_crypto::constant_eq(provided.as_bytes(), expected.as_bytes()) {
                return Ok(next.run(req).await);
            }
        }
    }
    // 2. A valid user session.
    if let Some(sess) = cookie_value(req.headers(), "nimbus_session") {
        if matches!(
            crate::users::validate_session(&st.pool, &sess).await,
            Ok(Some(_))
        ) {
            return Ok(next.run(req).await);
        }
    }
    // 3. Open instance: no admin token and no accounts (loopback/dev).
    let no_users = crate::users::user_count(&st.pool).await.unwrap_or(0) == 0;
    if st.admin_token.is_none() && no_users {
        return Ok(next.run(req).await);
    }
    Err(StatusCode::UNAUTHORIZED)
}

/// Largest file we attempt to index for semantic search (bytes).
const MAX_INDEX_BYTES: usize = 100_000;
/// How many top files to feed as context when chatting.
const CHAT_CONTEXT_FILES: usize = 3;
/// Max characters of each file included in the chat context.
const CHAT_EXCERPT_CHARS: usize = 2000;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/files", get(list_files))
        .route(
            "/api/files/*path",
            get(download_file).post(upload_file).delete(delete_file),
        )
        .route("/api/move", post(move_file))
        .route("/api/trash", get(list_trash))
        .route("/api/trash/restore", post(restore_trash))
        .route("/api/history/*path", get(file_history))
        .route("/api/activity", get(activity_feed))
        .route("/api/restore", post(restore_version))
        .route("/api/sync", post(sync_drive))
        .route("/api/usage", get(usage))
        .route("/api/folder-meta", get(folder_colors).put(set_folder_color))
        .route("/api/search", get(search_files))
        .route("/api/chat", post(chat))
        .route("/api/auth/status", get(auth_status))
        .route("/api/auth/device/start", post(auth_device_start))
        .route("/api/auth/device/poll", post(auth_device_poll))
        .route("/api/share", post(create_share_link))
        .route(
            "/api/share/*token",
            axum::routing::delete(revoke_share_link),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        // Public routes (added AFTER the auth layer): health, login, share.
        .route("/healthz", get(healthz))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/register", post(register))
        .route("/api/auth/me", get(me))
        .route("/s/*token", get(serve_share))
        .with_state(state)
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

fn session_cookie(token: &str) -> String {
    format!(
        "nimbus_session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
        crate::users::SESSION_TTL_SECS
    )
}

async fn login(
    State(st): State<AppState>,
    Json(cred): Json<Credentials>,
) -> Result<Response, StatusCode> {
    use axum::response::IntoResponse;
    let ok = crate::users::verify_login(&st.pool, &cred.username, &cred.password)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !ok {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let token = crate::users::create_session(&st.pool, &cred.username)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((
        [(axum::http::header::SET_COOKIE, session_cookie(&token))],
        Json(json!({ "user": cred.username })),
    )
        .into_response())
}

async fn logout(State(st): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    use axum::response::IntoResponse;
    if let Some(sess) = cookie_value(&headers, "nimbus_session") {
        let _ = crate::users::delete_session(&st.pool, &sess).await;
    }
    (
        [(
            axum::http::header::SET_COOKIE,
            "nimbus_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0".to_string(),
        )],
        StatusCode::NO_CONTENT,
    )
        .into_response()
}

/// Register the FIRST admin account. Allowed only when no `admin_token` is
/// configured (token-auth instances bootstrap admins via env), and only when no
/// account exists yet — enforced atomically to avoid a TOCTOU race.
async fn register(
    State(st): State<AppState>,
    Json(cred): Json<Credentials>,
) -> Result<Response, StatusCode> {
    use axum::response::IntoResponse;
    // When an admin token gates the API, accounts are seeded via env only.
    if st.admin_token.is_some() {
        return Err(StatusCode::FORBIDDEN);
    }
    let created = crate::users::register_first(&st.pool, &cred.username, &cred.password)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if !created {
        return Err(StatusCode::FORBIDDEN);
    }
    let token = crate::users::create_session(&st.pool, &cred.username)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((
        [(axum::http::header::SET_COOKIE, session_cookie(&token))],
        Json(json!({ "user": cred.username })),
    )
        .into_response())
}

async fn me(State(st): State<AppState>, headers: axum::http::HeaderMap) -> Json<Value> {
    let user = match cookie_value(&headers, "nimbus_session") {
        Some(sess) => crate::users::validate_session(&st.pool, &sess)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let requires_login = crate::users::user_count(&st.pool).await.unwrap_or(0) > 0;
    Json(json!({ "user": user, "requires_login": requires_login }))
}

/// Liveness/readiness probe for load balancers and uptime monitors.
async fn healthz() -> &'static str {
    "ok"
}

#[derive(Deserialize)]
struct ShareLinkRequest {
    path: String,
    password: Option<String>,
    expires_in_secs: Option<i64>,
}

async fn create_share_link(
    State(st): State<AppState>,
    Json(req): Json<ShareLinkRequest>,
) -> Result<Json<Value>, StatusCode> {
    let expires_at = req.expires_in_secs.map(|secs| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        now + secs
    });
    let token = crate::shares::create_share(
        &st.pool,
        &st.engine.drive_id(),
        &req.path,
        req.password.as_deref(),
        expires_at,
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        json!({ "token": token, "url": format!("/s/{token}") }),
    ))
}

async fn revoke_share_link(
    State(st): State<AppState>,
    Path(token): Path<String>,
) -> Result<StatusCode, StatusCode> {
    crate::shares::revoke_share(&st.pool, &token)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Minimal page prompting for a share password; it re-requests the file with
/// the password in the `X-Share-Password` header (never in the URL).
fn share_password_page(wrong: bool) -> axum::response::Response {
    use axum::response::{Html, IntoResponse};
    let msg = if wrong {
        "Wrong password — try again."
    } else {
        ""
    };
    let body = format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="referrer" content="no-referrer">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Protected file · Nimbus</title>
<style>body{{font-family:system-ui,sans-serif;background:#0f1115;color:#e6e8ec;display:flex;min-height:100vh;align-items:center;justify-content:center}}
.card{{background:#161922;border:1px solid #2a2f3a;border-radius:14px;padding:1.5rem;width:min(360px,90vw)}}
h1{{font-size:1.1rem;margin:0 0 1rem}}input{{width:100%;padding:.6rem;border-radius:9px;border:1px solid #2a2f3a;background:#1e222c;color:#e6e8ec;box-sizing:border-box}}
button{{margin-top:.8rem;width:100%;padding:.6rem;border:0;border-radius:9px;background:#6aa3ff;color:#07101f;font-weight:600;cursor:pointer}}
.msg{{color:#ff8a8a;font-size:.85rem;min-height:1.2em}}</style></head>
<body><div class="card"><h1>🌥️ This file is password-protected</h1>
<input id="pw" type="password" placeholder="Password" autofocus>
<div class="msg">{msg}</div>
<button onclick="go()">Download</button></div>
<script>
async function go(){{
  const pw=document.getElementById('pw').value;
  const r=await fetch(location.pathname,{{headers:{{'X-Share-Password':pw}}}});
  if(r.ok){{const b=await r.blob();const cd=r.headers.get('content-disposition')||'';
    const m=cd.match(/filename="?([^"]+)"?/);const a=document.createElement('a');
    a.href=URL.createObjectURL(b);a.download=m?m[1]:'download';a.click();}}
  else{{location.search='?e=1';}}
}}
document.getElementById('pw').addEventListener('keydown',e=>{{if(e.key==='Enter')go();}});
</script></body></html>"#
    );
    Html(body).into_response()
}

/// Public endpoint serving a shared file. The password (if any) is read from the
/// `X-Share-Password` header — never the URL, so it can't leak via logs,
/// browser history, or the Referer header. Direct browser hits get a prompt page.
async fn serve_share(
    State(st): State<AppState>,
    Path(token): Path<String>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use crate::shares::ShareError;
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    let pw = headers
        .get("x-share-password")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let path = match crate::shares::resolve_share(&st.pool, &token, pw.as_deref()).await {
        Ok(p) => p,
        // For a browser navigation (no header yet), show the prompt page.
        Err(ShareError::PasswordRequired) => return share_password_page(false),
        Err(ShareError::BadPassword) => return share_password_page(true),
        Err(ShareError::Expired) => return StatusCode::GONE.into_response(),
        Err(ShareError::NotFound) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let bytes = match st.engine.download(&path).await {
        Ok(b) => b,
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };
    let raw_name = path.rsplit('/').next().unwrap_or(&path);
    // Strip quotes / backslashes / control chars to prevent header or filename
    // injection via a crafted file name.
    let safe_name: String = raw_name
        .chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
        .collect();
    let safe_name = if safe_name.is_empty() {
        "download".to_string()
    } else {
        safe_name
    };
    (
        [
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{safe_name}\""),
            ),
            (header::REFERRER_POLICY, "no-referrer".to_string()),
        ],
        bytes,
    )
        .into_response()
}

/// Report whether in-app GitHub login (device flow) is available.
async fn auth_status(State(st): State<AppState>) -> Json<Value> {
    Json(json!({ "oauth_available": st.github_client_id.is_some() }))
}

/// Start the GitHub device flow; returns the user code + verification URL.
async fn auth_device_start(State(st): State<AppState>) -> Result<Json<DeviceCode>, StatusCode> {
    let client_id = st
        .github_client_id
        .as_ref()
        .ok_or(StatusCode::NOT_IMPLEMENTED)?;
    start_device_flow(GITHUB_OAUTH_BASE, client_id, "repo")
        .await
        .map(Json)
        .map_err(|_| StatusCode::BAD_GATEWAY)
}

#[derive(Deserialize)]
struct PollRequest {
    device_code: String,
}

/// Poll the device flow once. On success, hot-swap the token and persist it.
async fn auth_device_poll(
    State(st): State<AppState>,
    Json(req): Json<PollRequest>,
) -> Result<Json<Value>, StatusCode> {
    let client_id = st
        .github_client_id
        .as_ref()
        .ok_or(StatusCode::NOT_IMPLEMENTED)?;
    let result = poll_for_token(GITHUB_OAUTH_BASE, client_id, &req.device_code)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok(Json(match result {
        PollResult::Authorized(token) => accept_token(&st, token).await,
        PollResult::Pending => json!({ "status": "pending" }),
        PollResult::SlowDown => json!({ "status": "slow_down" }),
        PollResult::Denied => json!({ "status": "denied" }),
        PollResult::Failed(e) => json!({ "status": "error", "error": e }),
    }))
}

/// Verify a freshly obtained token belongs to the configured drive owner before
/// accepting it. This prevents an attacker from injecting *their own* token into
/// someone else's Nimbus instance via the unauthenticated device-flow endpoint.
async fn accept_token(st: &AppState, token: String) -> Value {
    let probe = GitHubClient::new(token.clone(), GITHUB_API_BASE.to_string());
    match probe.get_authenticated_user().await {
        Ok(login) if login.eq_ignore_ascii_case(&st.drive_owner) => {
            st.engine.set_github_token(&token);
            match crate::tokens::save_token(&st.pool, st.vault.as_ref(), &token).await {
                Ok(true) => {}
                Ok(false) => eprintln!(
                    "nimbus: OAuth token active (in-memory only; enable encryption to persist)"
                ),
                Err(e) => eprintln!("nimbus: failed to persist OAuth token: {e}"),
            }
            json!({ "status": "authorized", "user": login })
        }
        Ok(login) => json!({
            "status": "error",
            "error": format!("token belongs to '{login}', not the drive owner '{}'", st.drive_owner)
        }),
        Err(_) => json!({ "status": "error", "error": "could not verify token owner" }),
    }
}

#[derive(Deserialize)]
struct ListParams {
    /// When present, list the immediate children under this folder prefix.
    prefix: Option<String>,
}

async fn list_files(
    State(st): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<DriveFile>>, StatusCode> {
    let result = match params.prefix {
        Some(prefix) => st.engine.list_dir(&prefix).await,
        None => st.engine.list().await,
    };
    result
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Deserialize)]
struct DeleteParams {
    /// `true` = permanent delete; otherwise the file is moved to Trash.
    permanent: Option<bool>,
}

async fn delete_file(
    State(st): State<AppState>,
    Path(path): Path<String>,
    Query(params): Query<DeleteParams>,
) -> Result<StatusCode, StatusCode> {
    let permanent = params.permanent.unwrap_or(false);
    let result = if permanent {
        st.engine.delete(&path).await
    } else {
        st.engine.trash(&path).await
    };
    match result {
        Ok(()) => {
            if let Some(search) = &st.search {
                let _ = search.remove(&st.engine.drive_id(), &path).await;
            }
            Ok(StatusCode::NO_CONTENT)
        }
        Err(nimbus_core::NimbusError::NotFound(_)) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::BAD_GATEWAY),
    }
}

async fn list_trash(
    State(st): State<AppState>,
) -> Result<Json<Vec<nimbus_storage::TrashEntry>>, StatusCode> {
    st.engine
        .list_trash()
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Deserialize)]
struct RestoreRequest {
    trash_path: String,
}

async fn restore_trash(
    State(st): State<AppState>,
    Json(req): Json<RestoreRequest>,
) -> Result<StatusCode, StatusCode> {
    match st.engine.restore(&req.trash_path).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(nimbus_core::NimbusError::NotFound(_)) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::BAD_GATEWAY),
    }
}

#[derive(Deserialize)]
struct MoveRequest {
    from: String,
    to: String,
}

async fn move_file(
    State(st): State<AppState>,
    Json(req): Json<MoveRequest>,
) -> Result<StatusCode, StatusCode> {
    match st.engine.move_file(&req.from, &req.to).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(nimbus_core::NimbusError::NotFound(_)) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::BAD_GATEWAY),
    }
}

async fn upload_file(
    State(st): State<AppState>,
    Path(path): Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<DriveFile>, StatusCode> {
    let file = st
        .engine
        .upload(&path, &body)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    // Best-effort semantic indexing of text files. A failure here (e.g. the AI
    // provider is unreachable) must not fail the upload, but is logged.
    if let Some(search) = &st.search {
        if body.len() <= MAX_INDEX_BYTES {
            if let Ok(text) = std::str::from_utf8(&body) {
                if let Err(e) = search.index_file(&st.engine.drive_id(), &path, text).await {
                    eprintln!("nimbus: indexing failed for {path}: {e}");
                }
            }
        }
    }
    Ok(Json(file))
}

async fn download_file(
    State(st): State<AppState>,
    Path(path): Path<String>,
) -> Result<axum::body::Bytes, StatusCode> {
    match st.engine.download(&path).await {
        Ok(bytes) => Ok(bytes.into()),
        Err(nimbus_core::NimbusError::NotFound(_)) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::BAD_GATEWAY),
    }
}

async fn file_history(
    State(st): State<AppState>,
    Path(path): Path<String>,
) -> Result<Json<Vec<nimbus_github::CommitInfo>>, StatusCode> {
    st.engine
        .history(&path)
        .await
        .map(Json)
        .map_err(|_| StatusCode::BAD_GATEWAY)
}

#[derive(Deserialize)]
struct ActivityParams {
    /// Max number of recent events to return (default 50, capped server-side).
    limit: Option<u32>,
}

/// Drive-wide activity feed: the Git commit log, parsed into typed events
/// (upload/delete/trash/move/restore). This is Nimbus's audit trail — it lives
/// entirely in Git, so it can't be silently tampered with from the app.
async fn activity_feed(
    State(st): State<AppState>,
    Query(params): Query<ActivityParams>,
) -> Result<Json<Vec<crate::activity::ActivityEvent>>, StatusCode> {
    let limit = params.limit.unwrap_or(50).clamp(1, 100);
    let commits = st
        .engine
        .activity(limit)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok(Json(crate::activity::events_from_commits(commits)))
}

#[derive(Deserialize)]
struct RestoreVersionRequest {
    path: String,
    commit: String,
}

async fn restore_version(
    State(st): State<AppState>,
    Json(req): Json<RestoreVersionRequest>,
) -> Result<StatusCode, StatusCode> {
    match st.engine.restore_version(&req.path, &req.commit).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(nimbus_core::NimbusError::NotFound(_)) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::BAD_GATEWAY),
    }
}

async fn usage(State(st): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let (used, count) = st
        .engine
        .usage()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        json!({ "used": used, "count": count, "quota": st.engine.quota() }),
    ))
}

async fn folder_colors(State(st): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let pairs = crate::folders::colors(&st.pool, &st.engine.drive_id())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let map: serde_json::Map<String, Value> = pairs
        .into_iter()
        .map(|(p, c)| (p, Value::String(c)))
        .collect();
    Ok(Json(Value::Object(map)))
}

#[derive(Deserialize)]
struct FolderColorRequest {
    path: String,
    color: String,
}

async fn set_folder_color(
    State(st): State<AppState>,
    Json(req): Json<FolderColorRequest>,
) -> Result<StatusCode, StatusCode> {
    crate::folders::set_color(&st.pool, &st.engine.drive_id(), &req.path, &req.color)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn sync_drive(State(st): State<AppState>) -> Result<StatusCode, StatusCode> {
    st.engine
        .sync()
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|_| StatusCode::BAD_GATEWAY)
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    k: Option<usize>,
}

async fn search_files(
    State(st): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<SearchHit>>, StatusCode> {
    let search = st.search.as_ref().ok_or(StatusCode::NOT_IMPLEMENTED)?;
    search
        .search(&st.engine.drive_id(), &params.q, params.k.unwrap_or(10))
        .await
        .map(Json)
        .map_err(|_| StatusCode::BAD_GATEWAY)
}

#[derive(Deserialize)]
struct ChatRequest {
    question: String,
}

#[derive(Serialize)]
struct ChatResponse {
    answer: String,
    sources: Vec<String>,
}

/// "Chat with your files": retrieve the most relevant files (if search is
/// enabled), feed their excerpts as context, and ask the chat provider.
async fn chat(
    State(st): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, StatusCode> {
    let provider = st.chat.as_ref().ok_or(StatusCode::NOT_IMPLEMENTED)?;

    // Retrieve relevant files for context (best-effort).
    let mut context = String::new();
    let mut sources = Vec::new();
    if let Some(search) = &st.search {
        if let Ok(hits) = search
            .search(&st.engine.drive_id(), &req.question, CHAT_CONTEXT_FILES)
            .await
        {
            for hit in hits {
                if let Ok(bytes) = st.engine.download(&hit.path).await {
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        let excerpt: String = text.chars().take(CHAT_EXCERPT_CHARS).collect();
                        context.push_str(&format!("--- File: {}\n{}\n\n", hit.path, excerpt));
                        sources.push(hit.path);
                    }
                }
            }
        }
    }

    let system = "You are Nimbus, an assistant that answers questions about the \
        user's files. Use the provided file excerpts as context. If the context \
        is insufficient, say so. Cite file paths you used.";
    let user = if context.is_empty() {
        format!("Question: {}", req.question)
    } else {
        format!("File excerpts:\n{context}\nQuestion: {}", req.question)
    };

    let answer = provider
        .chat(system, &user)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok(Json(ChatResponse { answer, sources }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use nimbus_ai::{AiError, AiProvider, ChatProvider, Embedding};
    use serde_json::json;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path as wpath};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn memory_pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        pool
    }

    async fn test_state(gh_uri: String, search: Option<Arc<SearchIndex>>) -> AppState {
        let pool = memory_pool().await;
        let gh = nimbus_github::GitHubClient::new("tok", gh_uri);
        AppState {
            engine: Arc::new(StorageEngine::new(gh, pool.clone(), "me", "drive", "main")),
            search,
            chat: None,
            pool,
            github_client_id: None,
            drive_owner: "me".into(),
            vault: None,
            admin_token: None,
        }
    }

    /// Mount every endpoint an upload touches (blob + the commit dance).
    async fn mount_upload(server: &MockServer) {
        Mock::given(method("POST"))
            .and(wpath("/repos/me/drive/git/blobs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha":"s1"})))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/ref/heads/main"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"object":{"sha":"head1"}})),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/commits/head1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"tree":{"sha":"base"}})))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(wpath("/repos/me/drive/git/trees"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha":"tree1"})))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(wpath("/repos/me/drive/git/commits"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha":"commit1"})))
            .mount(server)
            .await;
        Mock::given(method("PATCH"))
            .and(wpath("/repos/me/drive/git/refs/heads/main"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ref":"refs/heads/main"})),
            )
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn upload_then_list_roundtrip() {
        let server = MockServer::start().await;
        mount_upload(&server).await;
        let app = router(test_state(server.uri(), None).await);

        let up = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/files/hello.txt")
                    .body(Body::from("hi"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(up.status(), StatusCode::OK);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/files")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let files: Vec<DriveFile> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "hello.txt");
    }

    #[tokio::test]
    async fn activity_feed_parses_commit_log() {
        use crate::activity::{Action, ActivityEvent};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/commits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "sha": "c3", "commit": { "message": "nimbus: upload a.txt", "author": { "date": "2026-06-02T10:00:00Z" } } },
                { "sha": "c2", "commit": { "message": "nimbus: move a.txt -> .nimbus-trash/1717/a.txt", "author": { "date": "2026-06-02T09:00:00Z" } } },
                { "sha": "c1", "commit": { "message": "initial", "author": { "date": "2026-06-01T08:00:00Z" } } }
            ])))
            .mount(&server)
            .await;
        let app = router(test_state(server.uri(), None).await);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/activity?limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let events: Vec<ActivityEvent> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].action, Action::Upload);
        assert_eq!(events[0].path, "a.txt");
        assert_eq!(events[1].action, Action::Trash);
        assert_eq!(events[1].path, "a.txt");
        assert_eq!(events[2].action, Action::Other);
    }

    #[tokio::test]
    async fn activity_feed_requires_auth_when_configured() {
        let server = MockServer::start().await;
        let mut st = test_state(server.uri(), None).await;
        st.admin_token = Some("secret".into());
        let app = router(st);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/activity")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn download_missing_returns_404() {
        let server = MockServer::start().await;
        let app = router(test_state(server.uri(), None).await);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/files/nope.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn first_register_succeeds_then_forbidden() {
        let server = MockServer::start().await;
        let app = router(test_state(server.uri(), None).await);
        let body = r#"{"username":"admin","password":"pw"}"#;
        let mk = || {
            Request::builder()
                .method("POST")
                .uri("/api/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap()
        };

        let r1 = app.clone().oneshot(mk()).await.unwrap();
        assert_eq!(r1.status(), StatusCode::OK);
        // Second registration is rejected once an account exists.
        let r2 = app.oneshot(mk()).await.unwrap();
        assert_eq!(r2.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn register_forbidden_when_admin_token_set() {
        let server = MockServer::start().await;
        let mut st = test_state(server.uri(), None).await;
        st.admin_token = Some("secret".into());
        let app = router(st);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/register")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"username":"x","password":"y"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn api_requires_token_when_configured() {
        let server = MockServer::start().await;
        let mut st = test_state(server.uri(), None).await;
        st.admin_token = Some("secret".into());
        let app = router(st);

        let unauth = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/files")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

        let ok = app
            .oneshot(
                Request::builder()
                    .uri("/api/files")
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn search_without_index_returns_501() {
        let server = MockServer::start().await;
        let app = router(test_state(server.uri(), None).await);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/search?q=cat")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    /// Deterministic fake: vector = [contains "cat", contains "dog"].
    struct FakeProvider;

    #[async_trait]
    impl AiProvider for FakeProvider {
        async fn embed(&self, texts: &[String]) -> std::result::Result<Vec<Embedding>, AiError> {
            Ok(texts
                .iter()
                .map(|t| {
                    let cat = if t.to_lowercase().contains("cat") {
                        1.0
                    } else {
                        0.0
                    };
                    let dog = if t.to_lowercase().contains("dog") {
                        1.0
                    } else {
                        0.0
                    };
                    vec![cat, dog]
                })
                .collect())
        }
    }

    #[tokio::test]
    async fn upload_indexes_and_search_finds_it() {
        let server = MockServer::start().await;
        mount_upload(&server).await;
        let pool = memory_pool().await;
        let gh = nimbus_github::GitHubClient::new("tok", server.uri());
        let engine = Arc::new(StorageEngine::new(gh, pool.clone(), "me", "drive", "main"));
        let search = Arc::new(SearchIndex::new(pool.clone(), Arc::new(FakeProvider)));
        let app = router(AppState {
            engine,
            search: Some(search),
            chat: None,
            pool,
            github_client_id: None,
            drive_owner: "me".into(),
            vault: None,
            admin_token: None,
        });

        // Upload a text file -> it should be indexed.
        let up = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/files/pet.txt")
                    .body(Body::from("my cat is fluffy"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(up.status(), StatusCode::OK);

        // Search should find it.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/search?q=cat")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let hits: Vec<SearchHit> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "pet.txt");
    }

    struct FakeChat;

    #[async_trait]
    impl ChatProvider for FakeChat {
        async fn chat(&self, _system: &str, _user: &str) -> std::result::Result<String, AiError> {
            Ok("stub answer".into())
        }
    }

    #[tokio::test]
    async fn chat_without_provider_returns_501() {
        let server = MockServer::start().await;
        let app = router(test_state(server.uri(), None).await);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"question":"hi"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn chat_with_provider_returns_answer() {
        let server = MockServer::start().await;
        let pool = memory_pool().await;
        let gh = nimbus_github::GitHubClient::new("tok", server.uri());
        let engine = Arc::new(StorageEngine::new(gh, pool.clone(), "me", "drive", "main"));
        let app = router(AppState {
            engine,
            search: None,
            chat: Some(Arc::new(FakeChat)),
            pool,
            github_client_id: None,
            drive_owner: "me".into(),
            vault: None,
            admin_token: None,
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"question":"what is in my files?"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["answer"], "stub answer");
    }
}

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use nimbus_core::DriveFile;
use nimbus_search::{SearchHit, SearchIndex};
use nimbus_storage::StorageEngine;
use serde::Deserialize;
use std::sync::Arc;

/// Shared application state: the storage engine plus an optional search index.
#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<StorageEngine>,
    pub search: Option<Arc<SearchIndex>>,
}

/// Largest file we attempt to index for semantic search (bytes).
const MAX_INDEX_BYTES: usize = 100_000;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/files", get(list_files))
        .route("/api/files/*path", get(download_file).post(upload_file))
        .route("/api/sync", post(sync_drive))
        .route("/api/search", get(search_files))
        .with_state(state)
}

async fn list_files(State(st): State<AppState>) -> Result<Json<Vec<DriveFile>>, StatusCode> {
    st.engine
        .list()
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use nimbus_ai::{AiError, AiProvider, Embedding};
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
            engine: Arc::new(StorageEngine::new(gh, pool, "me", "drive", "main")),
            search,
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
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"object":{"sha":"head1"}})))
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
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ref":"refs/heads/main"})))
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
            .oneshot(Request::builder().uri("/api/files").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let files: Vec<DriveFile> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "hello.txt");
    }

    #[tokio::test]
    async fn download_missing_returns_404() {
        let server = MockServer::start().await;
        let app = router(test_state(server.uri(), None).await);
        let resp = app
            .oneshot(Request::builder().uri("/api/files/nope.txt").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn search_without_index_returns_501() {
        let server = MockServer::start().await;
        let app = router(test_state(server.uri(), None).await);
        let resp = app
            .oneshot(Request::builder().uri("/api/search?q=cat").body(Body::empty()).unwrap())
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
                    let cat = if t.to_lowercase().contains("cat") { 1.0 } else { 0.0 };
                    let dog = if t.to_lowercase().contains("dog") { 1.0 } else { 0.0 };
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
        let search = Arc::new(SearchIndex::new(pool, Arc::new(FakeProvider)));
        let app = router(AppState { engine, search: Some(search) });

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
            .oneshot(Request::builder().uri("/api/search?q=cat").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let hits: Vec<SearchHit> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "pet.txt");
    }
}

use nimbus_ai::{AiProvider, AnthropicProvider, ChatProvider, OllamaProvider, OpenAiProvider};
use nimbus_crypto::Vault;
use nimbus_github::GitHubClient;
use nimbus_search::SearchIndex;
use nimbus_server::{cache, config::Config, routes, routes::AppState, tokens, vault};
use nimbus_storage::StorageEngine;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::from_lookup(|k| std::env::var(k).ok())?;
    let pool = cache::open(&cfg.database_url).await?;
    // Set up the vault first — it's needed to decrypt a stored token and is
    // shared with the request layer for encrypting newly obtained tokens.
    let vault: Option<Vault> = if let Some(passphrase) = &cfg.encryption_passphrase {
        let setup = vault::unlock_or_init(&pool, passphrase, cfg.recovery_key.as_deref()).await?;
        if let Some(recovery) = setup.new_recovery_key {
            emit_recovery_key(&recovery, cfg.recovery_key_out.as_deref())?;
        }
        println!("nimbus: client-side encryption ENABLED");
        Some(setup.vault)
    } else {
        println!("nimbus: encryption DISABLED (set NIMBUS_ENCRYPTION_PASSPHRASE to enable)");
        None
    };

    // Token precedence: env PAT, else a previously stored (encrypted) OAuth
    // token, else empty (the user can connect via the in-app device flow).
    let initial_token = match &cfg.github_token {
        Some(t) => t.clone(),
        None => tokens::load_token(&pool, vault.as_ref())
            .await?
            .unwrap_or_default(),
    };
    if initial_token.is_empty() {
        println!(
            "nimbus: no GitHub token yet — connect via the UI (needs NIMBUS_GITHUB_CLIENT_ID)"
        );
    }
    let gh = GitHubClient::new(initial_token, "https://api.github.com".to_string());

    let mut engine = StorageEngine::new(
        gh,
        pool.clone(),
        cfg.drive_owner.clone(),
        cfg.drive_repo.clone(),
        cfg.drive_branch.clone(),
    );
    if let Some(v) = &vault {
        engine = engine.with_vault(v.clone());
    }

    // Optional embedding provider drives semantic search.
    let embed_provider = build_embed_provider(&cfg);
    let search = embed_provider.map(|p| {
        println!("nimbus: semantic search ENABLED");
        Arc::new(SearchIndex::new(pool.clone(), p))
    });
    if search.is_none() {
        println!("nimbus: semantic search DISABLED (set NIMBUS_AI_PROVIDER to openai/ollama)");
    }

    // Optional chat provider drives "chat with your files".
    let chat = build_chat_provider(&cfg);
    if chat.is_some() {
        println!("nimbus: chat ENABLED");
    } else {
        println!("nimbus: chat DISABLED (set NIMBUS_AI_PROVIDER to openai/ollama/anthropic)");
    }

    let state = AppState {
        engine: Arc::new(engine),
        search,
        chat,
        pool: pool.clone(),
        github_client_id: cfg.github_client_id.clone(),
        drive_owner: cfg.drive_owner.clone(),
        vault: vault.clone(),
        admin_token: cfg.admin_token.clone(),
    };

    if cfg.admin_token.is_some() {
        println!("nimbus: API authentication ENABLED (NIMBUS_ADMIN_TOKEN)");
    }
    guard_bind(&cfg.bind_addr, cfg.admin_token.is_some())?;
    // API routes, with the frontend served as a fallback (same origin).
    let app = match &cfg.web_dir {
        Some(dir) => {
            println!("nimbus: serving UI from directory {dir}");
            routes::router(state).fallback_service(tower_http::services::ServeDir::new(dir))
        }
        None => {
            println!("nimbus: serving embedded UI");
            routes::router(state).fallback(nimbus_server::assets::static_handler)
        }
    };

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    println!("nimbus listening on http://{}", cfg.bind_addr);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build the embedding provider for semantic search, or `None`.
/// Anthropic has no embeddings API, so only openai/ollama qualify.
fn build_embed_provider(cfg: &Config) -> Option<Arc<dyn AiProvider>> {
    match cfg.ai_provider.as_deref() {
        Some("openai") => {
            let base = cfg
                .ai_base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com/v1".into());
            let key = cfg.ai_api_key.clone().unwrap_or_default();
            let model = cfg
                .ai_model
                .clone()
                .unwrap_or_else(|| "text-embedding-3-small".into());
            Some(Arc::new(OpenAiProvider::new(base, key, model)))
        }
        Some("ollama") => {
            let base = cfg
                .ai_base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434".into());
            let model = cfg
                .ai_model
                .clone()
                .unwrap_or_else(|| "nomic-embed-text".into());
            Some(Arc::new(OllamaProvider::new(base, model)))
        }
        _ => None,
    }
}

/// Build the chat provider for "chat with your files", or `None`.
fn build_chat_provider(cfg: &Config) -> Option<Arc<dyn ChatProvider>> {
    match cfg.ai_provider.as_deref() {
        Some("openai") => {
            let base = cfg
                .ai_base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com/v1".into());
            let key = cfg.ai_api_key.clone().unwrap_or_default();
            let model = cfg
                .ai_chat_model
                .clone()
                .unwrap_or_else(|| "gpt-4o-mini".into());
            Some(Arc::new(OpenAiProvider::new(base, key, model)))
        }
        Some("ollama") => {
            let base = cfg
                .ai_base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434".into());
            let model = cfg.ai_chat_model.clone().unwrap_or_else(|| "llama3".into());
            Some(Arc::new(OllamaProvider::new(base, model)))
        }
        Some("anthropic") => {
            let base = cfg
                .ai_base_url
                .clone()
                .unwrap_or_else(|| "https://api.anthropic.com".into());
            let key = cfg.ai_api_key.clone().unwrap_or_default();
            let model = cfg
                .ai_chat_model
                .clone()
                .unwrap_or_else(|| "claude-opus-4-8".into());
            Some(Arc::new(AnthropicProvider::new(base, key, model)))
        }
        _ => None,
    }
}

/// Refuse to bind to a non-loopback address unless API auth is configured.
/// On loopback we allow no-auth for convenient single-user/local use.
fn guard_bind(bind_addr: &str, auth_enabled: bool) -> anyhow::Result<()> {
    let host = bind_addr
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(bind_addr);
    let is_loopback =
        host.starts_with("127.") || host == "localhost" || host == "::1" || host == "[::1]";
    if !is_loopback && !auth_enabled {
        anyhow::bail!(
            "refusing to bind to non-loopback address {bind_addr} without authentication. \
             Set NIMBUS_ADMIN_TOKEN (and ideally put Nimbus behind a TLS reverse proxy), \
             or bind to 127.0.0.1 for local-only use."
        );
    }
    if !is_loopback {
        eprintln!(
            "nimbus: bound to {bind_addr} with auth enabled — ensure TLS via a reverse proxy."
        );
    }
    Ok(())
}

/// Emit the one-time recovery key. Prefer writing to a file (so it doesn't end
/// up in aggregated server logs); fall back to stdout with a clear warning.
/// This is the only time the key is ever shown.
fn emit_recovery_key(recovery: &str, out_path: Option<&str>) -> anyhow::Result<()> {
    if let Some(path) = out_path {
        std::fs::write(path, format!("{recovery}\n"))?;
        println!(
            "nimbus: recovery key written to {path} — store it safely, then delete it from disk."
        );
        return Ok(());
    }
    eprintln!("\n========================================================");
    eprintln!(" NIMBUS RECOVERY KEY (shown once — store it safely!)");
    eprintln!(" Without your passphrase AND this key, data is unrecoverable.");
    eprintln!(" Tip: set NIMBUS_RECOVERY_KEY_OUT to write this to a file instead.");
    eprintln!("--------------------------------------------------------");
    eprintln!(" {recovery}");
    eprintln!("========================================================\n");
    Ok(())
}

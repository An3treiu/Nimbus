use nimbus_ai::{AiProvider, OllamaProvider, OpenAiProvider};
use nimbus_github::GitHubClient;
use nimbus_search::SearchIndex;
use nimbus_server::{cache, config::Config, routes, routes::AppState, vault};
use nimbus_storage::StorageEngine;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::from_lookup(|k| std::env::var(k).ok())?;
    let pool = cache::open(&cfg.database_url).await?;
    let gh = GitHubClient::new(cfg.github_token.clone(), "https://api.github.com".to_string());

    let mut engine = StorageEngine::new(
        gh,
        pool.clone(),
        cfg.drive_owner.clone(),
        cfg.drive_repo.clone(),
        cfg.drive_branch.clone(),
    );

    if let Some(passphrase) = &cfg.encryption_passphrase {
        let setup = vault::unlock_or_init(&pool, passphrase, cfg.recovery_key.as_deref()).await?;
        if let Some(recovery) = setup.new_recovery_key {
            emit_recovery_key(&recovery, cfg.recovery_key_out.as_deref())?;
        }
        engine = engine.with_vault(setup.vault);
        println!("nimbus: client-side encryption ENABLED");
    } else {
        println!("nimbus: encryption DISABLED (set NIMBUS_ENCRYPTION_PASSPHRASE to enable)");
    }

    // Optional AI provider for semantic search.
    let provider = build_ai_provider(&cfg);
    let search = provider.map(|p| {
        println!("nimbus: semantic search ENABLED");
        Arc::new(SearchIndex::new(pool.clone(), p))
    });
    if search.is_none() {
        println!("nimbus: semantic search DISABLED (set NIMBUS_AI_PROVIDER to enable)");
    }

    let state = AppState {
        engine: Arc::new(engine),
        search,
    };
    // API routes, with the built frontend served as a fallback (same origin).
    let app = routes::router(state)
        .fallback_service(tower_http::services::ServeDir::new(&cfg.web_dir));
    println!("nimbus: serving UI from {}", cfg.web_dir);

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    println!("nimbus listening on http://{}", cfg.bind_addr);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build the configured AI provider, or `None` if AI is disabled.
fn build_ai_provider(cfg: &Config) -> Option<Arc<dyn AiProvider>> {
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

/// Emit the one-time recovery key. Prefer writing to a file (so it doesn't end
/// up in aggregated server logs); fall back to stdout with a clear warning.
/// This is the only time the key is ever shown.
fn emit_recovery_key(recovery: &str, out_path: Option<&str>) -> anyhow::Result<()> {
    if let Some(path) = out_path {
        std::fs::write(path, format!("{recovery}\n"))?;
        println!("nimbus: recovery key written to {path} — store it safely, then delete it from disk.");
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

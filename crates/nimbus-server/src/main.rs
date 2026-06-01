use nimbus_github::GitHubClient;
use nimbus_server::{cache, config::Config, routes};
use nimbus_storage::StorageEngine;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::from_lookup(|k| std::env::var(k).ok())?;
    let pool = cache::open(&cfg.database_url).await?;
    let gh = GitHubClient::new(cfg.github_token.clone(), "https://api.github.com".to_string());
    let engine = Arc::new(StorageEngine::new(
        gh,
        pool,
        cfg.drive_owner.clone(),
        cfg.drive_repo.clone(),
        cfg.drive_branch.clone(),
    ));
    let app = routes::router(engine);

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    println!("nimbus listening on http://{}", cfg.bind_addr);
    axum::serve(listener, app).await?;
    Ok(())
}

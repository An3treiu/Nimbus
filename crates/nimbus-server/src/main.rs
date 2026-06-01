use nimbus_github::GitHubClient;
use nimbus_server::{cache, config::Config, routes, vault};
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
        let setup = vault::unlock_or_init(&pool, passphrase).await?;
        if let Some(recovery) = setup.new_recovery_key {
            print_recovery_key(&recovery);
        }
        engine = engine.with_vault(setup.vault);
        println!("nimbus: client-side encryption ENABLED");
    } else {
        println!("nimbus: encryption DISABLED (set NIMBUS_ENCRYPTION_PASSPHRASE to enable)");
    }

    let app = routes::router(Arc::new(engine));

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    println!("nimbus listening on http://{}", cfg.bind_addr);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Print the one-time recovery key prominently. This is the only time it is
/// ever shown — it cannot be recovered from the stored data.
fn print_recovery_key(recovery: &str) {
    println!("\n========================================================");
    println!(" NIMBUS RECOVERY KEY (shown once — store it safely!)");
    println!(" Without your passphrase AND this key, data is unrecoverable.");
    println!("--------------------------------------------------------");
    println!(" {recovery}");
    println!("========================================================\n");
}

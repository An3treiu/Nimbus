use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

/// Open a pool and run migrations.
///
/// For in-memory databases (`url` contains ":memory:") we force a single
/// connection: each SQLite in-memory connection is an independent database,
/// so a multi-connection pool would migrate one and query another (empty) one.
pub async fn open(url: &str) -> anyhow::Result<SqlitePool> {
    let max = if url.contains(":memory:") { 1 } else { 5 };
    let pool = SqlitePoolOptions::new()
        .max_connections(max)
        .connect(url)
        .await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migration_creates_cached_files_table() {
        let pool = open("sqlite::memory:").await.unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='cached_files'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }
}

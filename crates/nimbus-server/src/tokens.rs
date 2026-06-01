//! Persistence for the OAuth-obtained GitHub access token.

use sqlx::SqlitePool;

/// Load the stored GitHub token, if any.
pub async fn load_token(pool: &SqlitePool) -> anyhow::Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT token FROM github_token WHERE id = 1")
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(t,)| t))
}

/// Persist the GitHub token (upsert into the single row).
pub async fn save_token(pool: &SqlitePool, token: &str) -> anyhow::Result<()> {
    sqlx::query("INSERT OR REPLACE INTO github_token (id, token) VALUES (1, ?)")
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn memory_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let pool = memory_pool().await;
        assert_eq!(load_token(&pool).await.unwrap(), None);
        save_token(&pool, "gho_abc").await.unwrap();
        assert_eq!(load_token(&pool).await.unwrap(), Some("gho_abc".into()));
        // Upsert replaces.
        save_token(&pool, "gho_new").await.unwrap();
        assert_eq!(load_token(&pool).await.unwrap(), Some("gho_new".into()));
    }
}

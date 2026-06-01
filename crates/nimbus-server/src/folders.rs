//! Per-folder UI metadata (SharePoint-style folder colors).

use sqlx::SqlitePool;

/// Set (or clear, if `color` is empty) the color for a folder path.
pub async fn set_color(
    pool: &SqlitePool,
    drive: &str,
    path: &str,
    color: &str,
) -> anyhow::Result<()> {
    if color.is_empty() {
        sqlx::query("DELETE FROM folder_meta WHERE drive = ? AND path = ?")
            .bind(drive)
            .bind(path)
            .execute(pool)
            .await?;
    } else {
        sqlx::query("INSERT OR REPLACE INTO folder_meta (drive, path, color) VALUES (?, ?, ?)")
            .bind(drive)
            .bind(path)
            .bind(color)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// All folder colors for a drive, as (path, color) pairs.
pub async fn colors(pool: &SqlitePool, drive: &str) -> anyhow::Result<Vec<(String, String)>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT path, color FROM folder_meta WHERE drive = ?")
            .bind(drive)
            .fetch_all(pool)
            .await?;
    Ok(rows)
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
    async fn set_get_and_clear_color() {
        let pool = memory_pool().await;
        set_color(&pool, "me/d", "Docs", "#ff8a3d").await.unwrap();
        set_color(&pool, "me/d", "Photos", "#6aa3ff").await.unwrap();
        let mut c = colors(&pool, "me/d").await.unwrap();
        c.sort();
        assert_eq!(
            c,
            vec![
                ("Docs".into(), "#ff8a3d".into()),
                ("Photos".into(), "#6aa3ff".into())
            ]
        );

        // Empty color clears it.
        set_color(&pool, "me/d", "Docs", "").await.unwrap();
        let c = colors(&pool, "me/d").await.unwrap();
        assert_eq!(c, vec![("Photos".into(), "#6aa3ff".into())]);
    }
}

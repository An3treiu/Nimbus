//! Semantic search over a drive's files.
//!
//! On indexing, a file's text is embedded (via the configured [`AiProvider`])
//! and the vector is cached in SQLite. A query is embedded the same way and
//! ranked against the cached vectors by cosine similarity. Embeddings are
//! computed from plaintext *before* any storage-layer encryption, so search
//! works even on encrypted drives.

use nimbus_ai::{cosine_similarity, AiProvider};
use nimbus_core::{NimbusError, Result};
use sqlx::SqlitePool;
use std::sync::Arc;

/// A single search hit: a file path and its similarity score in `[-1, 1]`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SearchHit {
    pub path: String,
    pub score: f32,
}

/// Stores and queries file embeddings for semantic search.
pub struct SearchIndex {
    pool: SqlitePool,
    provider: Arc<dyn AiProvider>,
}

impl SearchIndex {
    pub fn new(pool: SqlitePool, provider: Arc<dyn AiProvider>) -> Self {
        Self { pool, provider }
    }

    /// Embed `text` and cache its vector for `(drive, path)`.
    pub async fn index_file(&self, drive: &str, path: &str, text: &str) -> Result<()> {
        let vectors = self
            .provider
            .embed(&[text.to_string()])
            .await
            .map_err(|e| NimbusError::Ai(e.to_string()))?;
        let vector = vectors.into_iter().next().ok_or(NimbusError::Ai("no embedding returned".into()))?;
        let json = serde_json::to_string(&vector).map_err(|e| NimbusError::Ai(e.to_string()))?;
        sqlx::query("INSERT OR REPLACE INTO embeddings (drive, path, vector) VALUES (?, ?, ?)")
            .bind(drive)
            .bind(path)
            .bind(json)
            .execute(&self.pool)
            .await
            .map_err(|e| NimbusError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Remove a file's embedding (e.g. when it is deleted).
    pub async fn remove(&self, drive: &str, path: &str) -> Result<()> {
        sqlx::query("DELETE FROM embeddings WHERE drive = ? AND path = ?")
            .bind(drive)
            .bind(path)
            .execute(&self.pool)
            .await
            .map_err(|e| NimbusError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Embed `query` and return the top `top_k` files by cosine similarity,
    /// sorted descending.
    pub async fn search(&self, drive: &str, query: &str, top_k: usize) -> Result<Vec<SearchHit>> {
        let query_vec = self
            .provider
            .embed(&[query.to_string()])
            .await
            .map_err(|e| NimbusError::Ai(e.to_string()))?
            .into_iter()
            .next()
            .ok_or(NimbusError::Ai("no embedding returned".into()))?;

        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT path, vector FROM embeddings WHERE drive = ?")
                .bind(drive)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| NimbusError::Storage(e.to_string()))?;

        let mut hits: Vec<SearchHit> = rows
            .into_iter()
            .filter_map(|(path, json)| {
                let vec: Vec<f32> = serde_json::from_str(&json).ok()?;
                Some(SearchHit {
                    path,
                    score: cosine_similarity(&query_vec, &vec),
                })
            })
            .collect();

        // Sort by score descending (NaN-safe: treat unordered as equal).
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(top_k);
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use nimbus_ai::{AiError, Embedding};

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
    async fn search_ranks_relevant_file_first() {
        let index = SearchIndex::new(memory_pool().await, Arc::new(FakeProvider));
        index.index_file("d", "cat.txt", "i love my cat").await.unwrap();
        index.index_file("d", "dog.txt", "the dog barks").await.unwrap();

        let hits = index.search("d", "a fluffy cat", 5).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "cat.txt");
        assert!(hits[0].score > hits[1].score);
    }

    #[tokio::test]
    async fn top_k_limits_results() {
        let index = SearchIndex::new(memory_pool().await, Arc::new(FakeProvider));
        index.index_file("d", "cat.txt", "cat").await.unwrap();
        index.index_file("d", "dog.txt", "dog").await.unwrap();
        let hits = index.search("d", "cat", 1).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "cat.txt");
    }

    #[tokio::test]
    async fn search_is_scoped_per_drive() {
        let index = SearchIndex::new(memory_pool().await, Arc::new(FakeProvider));
        index.index_file("drive-a", "cat.txt", "cat").await.unwrap();
        let hits = index.search("drive-b", "cat", 5).await.unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn remove_drops_from_index() {
        let index = SearchIndex::new(memory_pool().await, Arc::new(FakeProvider));
        index.index_file("d", "cat.txt", "cat").await.unwrap();
        index.remove("d", "cat.txt").await.unwrap();
        let hits = index.search("d", "cat", 5).await.unwrap();
        assert!(hits.is_empty());
    }
}

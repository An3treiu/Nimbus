use nimbus_core::{DriveFile, FileKind, NimbusError, Result};
use nimbus_github::GitHubClient;
use sqlx::SqlitePool;

/// Orchestrates the drive model on top of GitHub blobs + the local cache.
pub struct StorageEngine {
    gh: GitHubClient,
    pool: SqlitePool,
    owner: String,
    repo: String,
}

impl StorageEngine {
    pub fn new(
        gh: GitHubClient,
        pool: SqlitePool,
        owner: impl Into<String>,
        repo: impl Into<String>,
    ) -> Self {
        Self {
            gh,
            pool,
            owner: owner.into(),
            repo: repo.into(),
        }
    }

    fn drive_key(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// Upload bytes to `path`: create a GitHub blob, then record it in the cache.
    pub async fn upload(&self, path: &str, bytes: &[u8]) -> Result<DriveFile> {
        let sha = self.gh.create_blob(&self.owner, &self.repo, bytes).await?;
        let file = DriveFile {
            path: path.to_string(),
            kind: FileKind::File,
            size: bytes.len() as u64,
            sha: Some(sha),
        };
        sqlx::query(
            "INSERT OR REPLACE INTO cached_files (drive, path, kind, size, sha) VALUES (?, ?, 'file', ?, ?)",
        )
        .bind(self.drive_key())
        .bind(&file.path)
        .bind(file.size as i64)
        .bind(file.sha.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|e| NimbusError::Storage(e.to_string()))?;
        Ok(file)
    }

    /// List cached files for this drive, sorted by path.
    pub async fn list(&self) -> Result<Vec<DriveFile>> {
        let rows: Vec<(String, String, i64, Option<String>)> = sqlx::query_as(
            "SELECT path, kind, size, sha FROM cached_files WHERE drive = ? ORDER BY path",
        )
        .bind(self.drive_key())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| NimbusError::Storage(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|(path, kind, size, sha)| DriveFile {
                path,
                kind: if kind == "folder" {
                    FileKind::Folder
                } else {
                    FileKind::File
                },
                size: size as u64,
                sha,
            })
            .collect())
    }

    /// Download a file's bytes: look up its SHA in the cache, then fetch the blob.
    pub async fn download(&self, path: &str) -> Result<Vec<u8>> {
        let sha: Option<String> = sqlx::query_scalar(
            "SELECT sha FROM cached_files WHERE drive = ? AND path = ?",
        )
        .bind(self.drive_key())
        .bind(path)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| NimbusError::Storage(e.to_string()))?
        .flatten();
        let sha = sha.ok_or_else(|| NimbusError::NotFound(path.to_string()))?;
        self.gh.get_blob(&self.owner, &self.repo, &sha).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path as wpath};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
    async fn upload_creates_blob_and_caches_row() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wpath("/repos/me/drive/git/blobs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha":"sha-1"})))
            .mount(&server)
            .await;

        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool.clone(), "me", "drive");

        let file = engine.upload("notes.md", b"hi").await.unwrap();
        assert_eq!(file.sha.as_deref(), Some("sha-1"));
        assert_eq!(file.size, 2);

        let cached: (String, i64) = sqlx::query_as(
            "SELECT sha, size FROM cached_files WHERE drive='me/drive' AND path='notes.md'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(cached.0, "sha-1");
        assert_eq!(cached.1, 2);
    }

    #[tokio::test]
    async fn list_returns_uploaded_files_sorted() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha":"s"})))
            .mount(&server)
            .await;
        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool, "me", "drive");

        engine.upload("b.txt", b"b").await.unwrap();
        engine.upload("a.txt", b"a").await.unwrap();

        let files = engine.list().await.unwrap();
        let names: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "b.txt"]);
    }

    #[tokio::test]
    async fn download_fetches_blob_by_cached_sha() {
        let server = MockServer::start().await;
        let encoded = nimbus_github::encode_blob(b"the body");
        Mock::given(method("POST"))
            .and(wpath("/repos/me/drive/git/blobs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha":"sha-x"})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/blobs/sha-x"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"content": encoded})))
            .mount(&server)
            .await;

        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool, "me", "drive");

        engine.upload("doc.txt", b"the body").await.unwrap();
        let bytes = engine.download("doc.txt").await.unwrap();
        assert_eq!(bytes, b"the body");
    }

    #[tokio::test]
    async fn download_missing_file_is_not_found() {
        let server = MockServer::start().await;
        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool, "me", "drive");
        let err = engine.download("ghost.txt").await.unwrap_err();
        assert!(matches!(err, NimbusError::NotFound(_)));
    }
}

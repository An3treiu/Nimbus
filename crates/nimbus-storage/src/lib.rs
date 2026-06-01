use nimbus_core::{DriveFile, FileKind, NimbusError, Result};
use nimbus_crypto::Vault;
use nimbus_github::GitHubClient;
use sqlx::SqlitePool;

/// Orchestrates the drive model on top of GitHub (durable commits) + the local cache.
///
/// When a [`Vault`] is attached, file bytes are encrypted before upload and
/// decrypted after download — GitHub only ever stores ciphertext.
pub struct StorageEngine {
    gh: GitHubClient,
    pool: SqlitePool,
    owner: String,
    repo: String,
    branch: String,
    vault: Option<Vault>,
}

impl StorageEngine {
    pub fn new(
        gh: GitHubClient,
        pool: SqlitePool,
        owner: impl Into<String>,
        repo: impl Into<String>,
        branch: impl Into<String>,
    ) -> Self {
        Self {
            gh,
            pool,
            owner: owner.into(),
            repo: repo.into(),
            branch: branch.into(),
            vault: None,
        }
    }

    /// Attach a vault so all uploads/downloads are transparently encrypted.
    pub fn with_vault(mut self, vault: Vault) -> Self {
        self.vault = Some(vault);
        self
    }

    /// Encrypt bytes for storage if a vault is attached, else pass through.
    fn seal(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        match &self.vault {
            Some(v) => v.seal(bytes).map_err(|e| NimbusError::Storage(e.to_string())),
            None => Ok(bytes.to_vec()),
        }
    }

    /// Decrypt stored bytes if a vault is attached, else pass through.
    fn open(&self, bytes: Vec<u8>) -> Result<Vec<u8>> {
        match &self.vault {
            Some(v) => v.open(&bytes).map_err(|e| NimbusError::Storage(e.to_string())),
            None => Ok(bytes),
        }
    }

    fn drive_key(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// Upload bytes to `path`: create a blob, commit it to the branch so it is
    /// durable, then record it in the cache.
    pub async fn upload(&self, path: &str, bytes: &[u8]) -> Result<DriveFile> {
        let stored = self.seal(bytes)?;
        let sha = self.gh.create_blob(&self.owner, &self.repo, &stored).await?;
        self.gh
            .commit_blob(
                &self.owner,
                &self.repo,
                &self.branch,
                path,
                &sha,
                &format!("nimbus: upload {path}"),
            )
            .await?;
        let file = DriveFile {
            path: path.to_string(),
            kind: FileKind::File,
            size: bytes.len() as u64,
            sha: Some(sha),
        };
        self.cache_put(&file).await?;
        Ok(file)
    }

    /// Refresh the cache from the branch's actual tree on GitHub.
    /// GitHub is the source of truth; this rebuilds the local view.
    pub async fn sync(&self) -> Result<()> {
        let files = self.gh.list_tree(&self.owner, &self.repo, &self.branch).await?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| NimbusError::Storage(e.to_string()))?;
        sqlx::query("DELETE FROM cached_files WHERE drive = ?")
            .bind(self.drive_key())
            .execute(&mut *tx)
            .await
            .map_err(|e| NimbusError::Storage(e.to_string()))?;
        for f in files {
            sqlx::query(
                "INSERT OR REPLACE INTO cached_files (drive, path, kind, size, sha) VALUES (?, ?, 'file', ?, ?)",
            )
            .bind(self.drive_key())
            .bind(&f.path)
            .bind(f.size as i64)
            .bind(&f.sha)
            .execute(&mut *tx)
            .await
            .map_err(|e| NimbusError::Storage(e.to_string()))?;
        }
        tx.commit()
            .await
            .map_err(|e| NimbusError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Insert or replace a single cached file row.
    async fn cache_put(&self, file: &DriveFile) -> Result<()> {
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
        Ok(())
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
        let sha: Option<String> =
            sqlx::query_scalar("SELECT sha FROM cached_files WHERE drive = ? AND path = ?")
                .bind(self.drive_key())
                .bind(path)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| NimbusError::Storage(e.to_string()))?
                .flatten();
        let sha = sha.ok_or_else(|| NimbusError::NotFound(path.to_string()))?;
        let raw = self.gh.get_blob(&self.owner, &self.repo, &sha).await?;
        self.open(raw)
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

    /// Mount every endpoint an `upload` touches (blob + the commit dance).
    /// The created blob SHA is `blob_sha`.
    async fn mount_upload(server: &MockServer, blob_sha: &str) {
        Mock::given(method("POST"))
            .and(wpath("/repos/me/drive/git/blobs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "sha": blob_sha })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/ref/heads/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "object": { "sha": "head1" } })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/commits/head1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "tree": { "sha": "base" } })))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(wpath("/repos/me/drive/git/trees"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "sha": "tree1" })))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(wpath("/repos/me/drive/git/commits"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "sha": "commit1" })))
            .mount(server)
            .await;
        Mock::given(method("PATCH"))
            .and(wpath("/repos/me/drive/git/refs/heads/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ref": "refs/heads/main" })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn upload_commits_and_caches_row() {
        let server = MockServer::start().await;
        mount_upload(&server, "sha-1").await;

        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool.clone(), "me", "drive", "main");

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
        mount_upload(&server, "s").await;
        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool, "me", "drive", "main");

        engine.upload("b.txt", b"b").await.unwrap();
        engine.upload("a.txt", b"a").await.unwrap();

        let files = engine.list().await.unwrap();
        let names: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "b.txt"]);
    }

    #[tokio::test]
    async fn download_fetches_blob_by_cached_sha() {
        let server = MockServer::start().await;
        mount_upload(&server, "sha-x").await;
        let encoded = nimbus_github::encode_blob(b"the body");
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/blobs/sha-x"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "content": encoded })))
            .mount(&server)
            .await;

        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool, "me", "drive", "main");

        engine.upload("doc.txt", b"the body").await.unwrap();
        let bytes = engine.download("doc.txt").await.unwrap();
        assert_eq!(bytes, b"the body");
    }

    #[tokio::test]
    async fn download_missing_file_is_not_found() {
        let server = MockServer::start().await;
        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool, "me", "drive", "main");
        let err = engine.download("ghost.txt").await.unwrap_err();
        assert!(matches!(err, NimbusError::NotFound(_)));
    }

    #[tokio::test]
    async fn sync_rebuilds_cache_from_github_tree() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/ref/heads/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "object": { "sha": "h1" } })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/commits/h1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "tree": { "sha": "tr1" } })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/trees/tr1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "tree": [
                    { "path": "x.txt", "type": "blob", "sha": "bx", "size": 5 },
                    { "path": "y.txt", "type": "blob", "sha": "by", "size": 7 }
                ]
            })))
            .mount(&server)
            .await;

        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool, "me", "drive", "main");

        engine.sync().await.unwrap();
        let files = engine.list().await.unwrap();
        let names: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, vec!["x.txt", "y.txt"]);
        assert_eq!(files[0].sha.as_deref(), Some("bx"));
    }

    #[tokio::test]
    async fn upload_with_vault_sends_ciphertext_to_github() {
        let server = MockServer::start().await;
        mount_upload(&server, "encsha").await;

        let vault = nimbus_crypto::Vault::new(nimbus_crypto::generate_key());
        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool, "me", "drive", "main").with_vault(vault.clone());

        engine.upload("secret.txt", b"classified").await.unwrap();

        // Inspect what was actually POSTed to the blob endpoint.
        let reqs = server.received_requests().await.unwrap();
        let blob_req = reqs
            .iter()
            .find(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/git/blobs"))
            .expect("a blob POST was made");
        let body: serde_json::Value = serde_json::from_slice(&blob_req.body).unwrap();
        let content_b64 = body["content"].as_str().unwrap();
        let ciphertext = nimbus_github::decode_blob(content_b64).unwrap();

        assert_ne!(ciphertext, b"classified", "GitHub must never see plaintext");
        assert_eq!(vault.open(&ciphertext).unwrap(), b"classified");
    }

    #[tokio::test]
    async fn download_with_vault_decrypts() {
        let server = MockServer::start().await;
        let vault = nimbus_crypto::Vault::new(nimbus_crypto::generate_key());
        let ciphertext = vault.seal(b"hello enc").unwrap();
        let encoded = nimbus_github::encode_blob(&ciphertext);
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/blobs/csha"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "content": encoded })))
            .mount(&server)
            .await;

        let pool = memory_pool().await;
        sqlx::query(
            "INSERT INTO cached_files (drive, path, kind, size, sha) VALUES ('me/drive','f.bin','file',9,'csha')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let gh = GitHubClient::new("tok", server.uri());
        let engine = StorageEngine::new(gh, pool, "me", "drive", "main").with_vault(vault);

        let bytes = engine.download("f.bin").await.unwrap();
        assert_eq!(bytes, b"hello enc");
    }
}

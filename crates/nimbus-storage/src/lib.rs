use nimbus_core::{DriveFile, FileKind, NimbusError, Result};
use nimbus_crypto::Vault;
use nimbus_github::{GitHubClient, TreeChange};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::BTreeSet;

/// Default chunking threshold: files larger than this are split. Kept below
/// GitHub's ~100 MB blob limit, allowing for base64 expansion (~33%).
const DEFAULT_CHUNK_SIZE: usize = 50 * 1024 * 1024;

/// Marker prefixing a chunk manifest blob, so download can tell a manifest from
/// a regular file. Chosen to be vanishingly unlikely to start a real file.
const MANIFEST_MAGIC: &[u8] = b"NIMBUSv1CHUNKED\n";

/// Path prefix under which trashed files live in the repo.
const TRASH_PREFIX: &str = ".nimbus-trash/";

/// A trashed file: where it now lives, where it came from, and when it was deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrashEntry {
    pub trash_path: String,
    pub original_path: String,
    pub deleted_at: i64,
}

/// Current unix time in seconds (0 if the clock is before the epoch).
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Describes a large file split across multiple chunk blobs.
#[derive(Serialize, Deserialize)]
struct Manifest {
    /// Total size of the reassembled (plaintext) file.
    size: u64,
    /// Chunk blob SHAs, in order.
    chunks: Vec<String>,
}

/// Orchestrates the drive model on top of GitHub (durable commits) + the local cache.
///
/// When a [`Vault`] is attached, file bytes are encrypted before upload and
/// decrypted after download — GitHub only ever stores ciphertext. Files larger
/// than `chunk_size` are split into multiple blobs plus a manifest.
pub struct StorageEngine {
    gh: GitHubClient,
    pool: SqlitePool,
    owner: String,
    repo: String,
    branch: String,
    vault: Option<Vault>,
    chunk_size: usize,
    quota_bytes: Option<u64>,
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
            chunk_size: DEFAULT_CHUNK_SIZE,
            quota_bytes: None,
        }
    }

    /// Attach a vault so all uploads/downloads are transparently encrypted.
    pub fn with_vault(mut self, vault: Vault) -> Self {
        self.vault = Some(vault);
        self
    }

    /// Override the chunking threshold (bytes). Mainly for tests.
    pub fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size.max(1);
        self
    }

    /// Set a storage quota (bytes); uploads that would exceed it are rejected.
    pub fn with_quota(mut self, quota_bytes: Option<u64>) -> Self {
        self.quota_bytes = quota_bytes;
        self
    }

    /// Current usage: (bytes used, file count), excluding trashed files.
    pub async fn usage(&self) -> Result<(u64, u64)> {
        let row: (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(size), 0), COUNT(*) FROM cached_files \
             WHERE drive = ? AND path NOT LIKE ?",
        )
        .bind(self.drive_key())
        .bind(format!("{TRASH_PREFIX}%"))
        .fetch_one(&self.pool)
        .await
        .map_err(|e| NimbusError::Storage(e.to_string()))?;
        Ok((row.0 as u64, row.1 as u64))
    }

    /// The configured quota in bytes, if any.
    pub fn quota(&self) -> Option<u64> {
        self.quota_bytes
    }

    /// Encrypt bytes for storage if a vault is attached, else pass through.
    /// The file path is bound as associated data (anti-substitution).
    fn seal(&self, path: &str, bytes: &[u8]) -> Result<Vec<u8>> {
        match &self.vault {
            Some(v) => v
                .seal(path.as_bytes(), bytes)
                .map_err(|e| NimbusError::Storage(e.to_string())),
            None => Ok(bytes.to_vec()),
        }
    }

    /// Decrypt stored bytes if a vault is attached, else pass through.
    fn open(&self, path: &str, bytes: Vec<u8>) -> Result<Vec<u8>> {
        match &self.vault {
            Some(v) => v
                .open(path.as_bytes(), &bytes)
                .map_err(|e| NimbusError::Storage(e.to_string())),
            None => Ok(bytes),
        }
    }

    fn drive_key(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// Stable identifier for this drive (`owner/repo`), used as a cache/search key.
    pub fn drive_id(&self) -> String {
        self.drive_key()
    }

    /// Replace the GitHub access token at runtime (after OAuth login).
    pub fn set_github_token(&self, token: impl Into<String>) {
        self.gh.set_token(token);
    }

    /// Store file content as one or more blobs and return the SHA to commit at
    /// `path`. Small files become a single (optionally encrypted) blob; large
    /// files are split into chunk blobs plus a manifest blob.
    async fn store_content(&self, path: &str, bytes: &[u8]) -> Result<String> {
        if bytes.len() <= self.chunk_size {
            let stored = self.seal(path, bytes)?;
            return self.gh.create_blob(&self.owner, &self.repo, &stored).await;
        }
        let mut chunks = Vec::new();
        for chunk in bytes.chunks(self.chunk_size) {
            let stored = self.seal(path, chunk)?;
            chunks.push(
                self.gh
                    .create_blob(&self.owner, &self.repo, &stored)
                    .await?,
            );
        }
        let manifest = Manifest {
            size: bytes.len() as u64,
            chunks,
        };
        let mut blob = MANIFEST_MAGIC.to_vec();
        blob.extend_from_slice(
            &serde_json::to_vec(&manifest).map_err(|e| NimbusError::Storage(e.to_string()))?,
        );
        self.gh.create_blob(&self.owner, &self.repo, &blob).await
    }

    /// Upload bytes to `path`: create a blob, commit it to the branch so it is
    /// durable, then record it in the cache.
    pub async fn upload(&self, path: &str, bytes: &[u8]) -> Result<DriveFile> {
        if let Some(quota) = self.quota_bytes {
            let (used, _) = self.usage().await?;
            if used.saturating_add(bytes.len() as u64) > quota {
                return Err(NimbusError::Storage(format!(
                    "storage quota exceeded ({used} + {} > {quota} bytes)",
                    bytes.len()
                )));
            }
        }
        let sha = self.store_content(path, bytes).await?;
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
        let files = self
            .gh
            .list_tree(&self.owner, &self.repo, &self.branch)
            .await?;
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
            "SELECT path, kind, size, sha FROM cached_files WHERE drive = ? \
             AND path NOT LIKE ? ORDER BY path",
        )
        .bind(self.drive_key())
        .bind(format!("{TRASH_PREFIX}%"))
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

        if raw.starts_with(MANIFEST_MAGIC) {
            // Chunked file: fetch and decrypt each chunk, then concatenate.
            let manifest: Manifest = serde_json::from_slice(&raw[MANIFEST_MAGIC.len()..])
                .map_err(|e| NimbusError::Storage(e.to_string()))?;
            let mut out = Vec::with_capacity(manifest.size as usize);
            for chunk_sha in &manifest.chunks {
                let chunk_raw = self.gh.get_blob(&self.owner, &self.repo, chunk_sha).await?;
                out.extend(self.open(path, chunk_raw)?);
            }
            Ok(out)
        } else {
            self.open(path, raw)
        }
    }

    /// Delete a file: commit its removal from the branch and drop it from the cache.
    pub async fn delete(&self, path: &str) -> Result<()> {
        let changes = [TreeChange {
            path: path.to_string(),
            blob_sha: None,
        }];
        self.gh
            .commit_changes(
                &self.owner,
                &self.repo,
                &self.branch,
                &changes,
                &format!("nimbus: delete {path}"),
            )
            .await?;
        sqlx::query("DELETE FROM cached_files WHERE drive = ? AND path = ?")
            .bind(self.drive_key())
            .bind(path)
            .execute(&self.pool)
            .await
            .map_err(|e| NimbusError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Move/rename a file from `from` to `to`.
    ///
    /// With encryption on, the ciphertext is bound to its path (AAD), so we must
    /// re-encrypt under the new path. Without encryption, we reuse the blob SHA.
    pub async fn move_file(&self, from: &str, to: &str) -> Result<()> {
        let (new_sha, size) = if self.vault.is_some() {
            // Re-encrypt under the new path.
            let bytes = self.download(from).await?;
            let sha = self.store_content(to, &bytes).await?;
            (sha, bytes.len() as u64)
        } else {
            let row: Option<(String, i64)> =
                sqlx::query_as("SELECT sha, size FROM cached_files WHERE drive = ? AND path = ?")
                    .bind(self.drive_key())
                    .bind(from)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|e| NimbusError::Storage(e.to_string()))?;
            let (sha, size) = row.ok_or_else(|| NimbusError::NotFound(from.to_string()))?;
            (sha, size as u64)
        };

        let changes = [
            TreeChange {
                path: from.to_string(),
                blob_sha: None,
            },
            TreeChange {
                path: to.to_string(),
                blob_sha: Some(new_sha.clone()),
            },
        ];
        self.gh
            .commit_changes(
                &self.owner,
                &self.repo,
                &self.branch,
                &changes,
                &format!("nimbus: move {from} -> {to}"),
            )
            .await?;

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| NimbusError::Storage(e.to_string()))?;
        sqlx::query("DELETE FROM cached_files WHERE drive = ? AND path = ?")
            .bind(self.drive_key())
            .bind(from)
            .execute(&mut *tx)
            .await
            .map_err(|e| NimbusError::Storage(e.to_string()))?;
        sqlx::query(
            "INSERT OR REPLACE INTO cached_files (drive, path, kind, size, sha) VALUES (?, ?, 'file', ?, ?)",
        )
        .bind(self.drive_key())
        .bind(to)
        .bind(size as i64)
        .bind(&new_sha)
        .execute(&mut *tx)
        .await
        .map_err(|e| NimbusError::Storage(e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| NimbusError::Storage(e.to_string()))?;
        Ok(())
    }

    /// List the immediate children (files + subfolders) under `prefix`.
    /// `prefix` "" lists the root. Folders are derived from path segments.
    pub async fn list_dir(&self, prefix: &str) -> Result<Vec<DriveFile>> {
        let norm = if prefix.is_empty() || prefix.ends_with('/') {
            prefix.to_string()
        } else {
            format!("{prefix}/")
        };
        let rows: Vec<(String, i64, Option<String>)> = sqlx::query_as(
            "SELECT path, size, sha FROM cached_files WHERE drive = ? AND path LIKE ? \
             AND path NOT LIKE ? ORDER BY path",
        )
        .bind(self.drive_key())
        .bind(format!("{norm}%"))
        .bind(format!("{TRASH_PREFIX}%"))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| NimbusError::Storage(e.to_string()))?;

        let mut folders: BTreeSet<String> = BTreeSet::new();
        let mut files: Vec<DriveFile> = Vec::new();
        for (path, size, sha) in rows {
            let rest = &path[norm.len()..];
            if rest.is_empty() {
                continue;
            }
            match rest.find('/') {
                Some(idx) => {
                    folders.insert(rest[..idx].to_string());
                }
                None => files.push(DriveFile {
                    path: path.clone(),
                    kind: FileKind::File,
                    size: size as u64,
                    sha,
                }),
            }
        }
        let mut out: Vec<DriveFile> = folders
            .into_iter()
            .map(|name| DriveFile {
                path: format!("{norm}{name}"),
                kind: FileKind::Folder,
                size: 0,
                sha: None,
            })
            .collect();
        out.extend(files);
        Ok(out)
    }

    /// The commit history for a file (newest first).
    pub async fn history(&self, path: &str) -> Result<Vec<nimbus_github::CommitInfo>> {
        self.gh
            .list_commits(&self.owner, &self.repo, &self.branch, path)
            .await
    }

    /// Restore `path` to the version it had at `commit_sha` (a new commit that
    /// re-points the path to the historical blob). Works with encryption since
    /// the path — and thus the AAD — is unchanged.
    pub async fn restore_version(&self, path: &str, commit_sha: &str) -> Result<()> {
        let file = self
            .gh
            .file_at_commit(&self.owner, &self.repo, commit_sha, path)
            .await?
            .ok_or_else(|| NimbusError::NotFound(format!("{path}@{commit_sha}")))?;
        let changes = [TreeChange {
            path: path.to_string(),
            blob_sha: Some(file.sha.clone()),
        }];
        self.gh
            .commit_changes(
                &self.owner,
                &self.repo,
                &self.branch,
                &changes,
                &format!("nimbus: restore {path} to {commit_sha}"),
            )
            .await?;
        self.cache_put(&DriveFile {
            path: path.to_string(),
            kind: FileKind::File,
            size: file.size,
            sha: Some(file.sha),
        })
        .await?;
        Ok(())
    }

    /// Move a file to the trash (a `.nimbus-trash/<ts>/<path>` location) and
    /// record it so it can be listed/restored/purged later.
    pub async fn trash(&self, path: &str) -> Result<()> {
        let ts = now_secs();
        let trash_path = format!("{TRASH_PREFIX}{ts}/{path}");
        self.move_file(path, &trash_path).await?;
        sqlx::query(
            "INSERT OR REPLACE INTO trash (drive, trash_path, original_path, deleted_at) VALUES (?, ?, ?, ?)",
        )
        .bind(self.drive_key())
        .bind(&trash_path)
        .bind(path)
        .bind(ts)
        .execute(&self.pool)
        .await
        .map_err(|e| NimbusError::Storage(e.to_string()))?;
        Ok(())
    }

    /// List trashed files, most recently deleted first.
    pub async fn list_trash(&self) -> Result<Vec<TrashEntry>> {
        let rows: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT trash_path, original_path, deleted_at FROM trash WHERE drive = ? \
             ORDER BY deleted_at DESC",
        )
        .bind(self.drive_key())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| NimbusError::Storage(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|(trash_path, original_path, deleted_at)| TrashEntry {
                trash_path,
                original_path,
                deleted_at,
            })
            .collect())
    }

    /// Restore a trashed file back to its original path.
    pub async fn restore(&self, trash_path: &str) -> Result<()> {
        let original: Option<String> = sqlx::query_scalar(
            "SELECT original_path FROM trash WHERE drive = ? AND trash_path = ?",
        )
        .bind(self.drive_key())
        .bind(trash_path)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| NimbusError::Storage(e.to_string()))?;
        let original = original.ok_or_else(|| NimbusError::NotFound(trash_path.to_string()))?;
        self.move_file(trash_path, &original).await?;
        sqlx::query("DELETE FROM trash WHERE drive = ? AND trash_path = ?")
            .bind(self.drive_key())
            .bind(trash_path)
            .execute(&self.pool)
            .await
            .map_err(|e| NimbusError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Permanently delete trashed entries older than `retention_secs`.
    /// Returns the number of entries purged.
    pub async fn purge_expired(&self, retention_secs: i64) -> Result<u64> {
        let cutoff = now_secs() - retention_secs;
        let expired: Vec<String> =
            sqlx::query_scalar("SELECT trash_path FROM trash WHERE drive = ? AND deleted_at < ?")
                .bind(self.drive_key())
                .bind(cutoff)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| NimbusError::Storage(e.to_string()))?;

        let mut purged = 0;
        for trash_path in expired {
            self.delete(&trash_path).await?;
            sqlx::query("DELETE FROM trash WHERE drive = ? AND trash_path = ?")
                .bind(self.drive_key())
                .bind(&trash_path)
                .execute(&self.pool)
                .await
                .map_err(|e| NimbusError::Storage(e.to_string()))?;
            purged += 1;
        }
        Ok(purged)
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
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "object": { "sha": "head1" } })),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/commits/head1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "tree": { "sha": "base" } })),
            )
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
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "ref": "refs/heads/main" })),
            )
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
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "object": { "sha": "h1" } })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/commits/h1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "tree": { "sha": "tr1" } })),
            )
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
        // The ciphertext is bound to the path via AAD.
        assert_eq!(
            vault.open(b"secret.txt", &ciphertext).unwrap(),
            b"classified"
        );
    }

    #[tokio::test]
    async fn download_with_vault_decrypts() {
        let server = MockServer::start().await;
        let vault = nimbus_crypto::Vault::new(nimbus_crypto::generate_key());
        let ciphertext = vault.seal(b"f.bin", b"hello enc").unwrap();
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

    #[tokio::test]
    async fn upload_chunks_large_file_and_writes_manifest() {
        let server = MockServer::start().await;
        mount_upload(&server, "blob").await;
        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        // chunk_size = 4 -> "0123456789" (10 bytes) becomes 3 chunks + 1 manifest.
        let engine = StorageEngine::new(gh, pool, "me", "drive", "main").with_chunk_size(4);

        engine.upload("big.bin", b"0123456789").await.unwrap();

        let reqs = server.received_requests().await.unwrap();
        let blob_posts: Vec<_> = reqs
            .iter()
            .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/git/blobs"))
            .collect();
        assert_eq!(blob_posts.len(), 4, "3 chunks + 1 manifest");

        // Find the manifest among the posted blobs.
        let manifest = blob_posts.iter().find_map(|r| {
            let body: serde_json::Value = serde_json::from_slice(&r.body).ok()?;
            let content = nimbus_github::decode_blob(body["content"].as_str()?).ok()?;
            if content.starts_with(MANIFEST_MAGIC) {
                serde_json::from_slice::<Manifest>(&content[MANIFEST_MAGIC.len()..]).ok()
            } else {
                None
            }
        });
        let manifest = manifest.expect("a manifest blob was posted");
        assert_eq!(manifest.size, 10);
        assert_eq!(manifest.chunks.len(), 3);
    }

    #[tokio::test]
    async fn download_reassembles_chunks_from_manifest() {
        let server = MockServer::start().await;
        let manifest = Manifest {
            size: 6,
            chunks: vec!["c0".into(), "c1".into()],
        };
        let mut manifest_blob = MANIFEST_MAGIC.to_vec();
        manifest_blob.extend_from_slice(&serde_json::to_vec(&manifest).unwrap());

        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/blobs/man"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({ "content": nimbus_github::encode_blob(&manifest_blob) }),
                ),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/blobs/c0"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "content": nimbus_github::encode_blob(b"AAAA") })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/blobs/c1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "content": nimbus_github::encode_blob(b"BB") })),
            )
            .mount(&server)
            .await;

        let pool = memory_pool().await;
        sqlx::query(
            "INSERT INTO cached_files (drive, path, kind, size, sha) VALUES ('me/drive','big.bin','file',6,'man')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let gh = GitHubClient::new("tok", server.uri());
        let engine = StorageEngine::new(gh, pool, "me", "drive", "main");
        let bytes = engine.download("big.bin").await.unwrap();
        assert_eq!(bytes, b"AAAABB");
    }

    #[tokio::test]
    async fn delete_removes_from_cache_and_commits() {
        let server = MockServer::start().await;
        mount_upload(&server, "s1").await;
        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool, "me", "drive", "main");

        engine.upload("a.txt", b"x").await.unwrap();
        assert_eq!(engine.list().await.unwrap().len(), 1);
        engine.delete("a.txt").await.unwrap();
        assert!(engine.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn move_renames_in_cache() {
        let server = MockServer::start().await;
        mount_upload(&server, "s1").await;
        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool, "me", "drive", "main");

        engine.upload("a.txt", b"x").await.unwrap();
        engine.move_file("a.txt", "docs/b.txt").await.unwrap();
        let files = engine.list().await.unwrap();
        let names: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, vec!["docs/b.txt"]);
    }

    #[tokio::test]
    async fn usage_sums_sizes_and_enforces_quota() {
        let server = MockServer::start().await;
        mount_upload(&server, "s1").await;
        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool, "me", "drive", "main").with_quota(Some(5));

        engine.upload("a.txt", b"abc").await.unwrap(); // 3 bytes, ok
        let (used, count) = engine.usage().await.unwrap();
        assert_eq!(used, 3);
        assert_eq!(count, 1);

        // Next 3 bytes would total 6 > quota 5 -> rejected.
        let err = engine.upload("b.txt", b"xyz").await.unwrap_err();
        assert!(matches!(err, NimbusError::Storage(_)));
        // Usage unchanged.
        assert_eq!(engine.usage().await.unwrap().0, 3);
    }

    #[tokio::test]
    async fn trash_then_restore_round_trips() {
        let server = MockServer::start().await;
        mount_upload(&server, "s1").await;
        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool, "me", "drive", "main");

        engine.upload("a.txt", b"x").await.unwrap();
        engine.trash("a.txt").await.unwrap();

        // Trashed files don't show in the normal listing.
        assert!(engine.list().await.unwrap().is_empty());
        let trash = engine.list_trash().await.unwrap();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].original_path, "a.txt");
        assert!(trash[0].trash_path.starts_with(".nimbus-trash/"));

        // Restore brings it back and empties the trash.
        engine.restore(&trash[0].trash_path).await.unwrap();
        let files = engine.list().await.unwrap();
        assert_eq!(
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec!["a.txt"]
        );
        assert!(engine.list_trash().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_dir_derives_folders_and_files() {
        let pool = memory_pool().await;
        for (p, s) in [("readme.md", 3), ("docs/a.md", 5), ("docs/sub/c.md", 7)] {
            sqlx::query("INSERT INTO cached_files (drive, path, kind, size, sha) VALUES ('me/drive', ?, 'file', ?, 's')")
                .bind(p)
                .bind(s)
                .execute(&pool)
                .await
                .unwrap();
        }
        let gh = GitHubClient::new("tok", "http://unused");
        let engine = StorageEngine::new(gh, pool, "me", "drive", "main");

        // Root: folder "docs" + file "readme.md".
        let root = engine.list_dir("").await.unwrap();
        let root_names: Vec<_> = root.iter().map(|f| (f.path.as_str(), &f.kind)).collect();
        assert_eq!(
            root_names,
            vec![("docs", &FileKind::Folder), ("readme.md", &FileKind::File)]
        );

        // Inside docs: folder "docs/sub" + file "docs/a.md".
        let docs = engine.list_dir("docs").await.unwrap();
        let docs_names: Vec<_> = docs.iter().map(|f| (f.path.as_str(), &f.kind)).collect();
        assert_eq!(
            docs_names,
            vec![
                ("docs/sub", &FileKind::Folder),
                ("docs/a.md", &FileKind::File)
            ]
        );
    }
}

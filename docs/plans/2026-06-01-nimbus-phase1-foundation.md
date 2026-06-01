# Nimbus Phase 1 — Foundation & GitHub Storage Core (Implementation Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a working "walking skeleton" of Nimbus: a Rust/Axum server that, given a GitHub token, can connect a repo as a drive and list / upload / download small files through GitHub's Git Data API, backed by a local SQLite cache.

**Architecture:** A Cargo workspace with focused crates (`nimbus-core`, `nimbus-github`, `nimbus-storage`, `nimbus-server`). The server exposes a small JSON HTTP API. GitHub is the source of truth; SQLite is a rebuildable cache. We build vertically: every milestone produces software you can run and test.

**Tech Stack:** Rust (stable), Axum (HTTP), Tokio (async), reqwest (GitHub client), sqlx + SQLite (cache), serde (JSON), base64, anyhow/thiserror (errors). Tests use Rust's built-in test harness + `wiremock` for mocking GitHub.

**Phasing note:** This is Phase 1 of the MVP. Later phases — client-side encryption, large-file chunking, AI provider abstraction + semantic search, file preview, GitHub OAuth UI, and the SvelteKit frontend — each get their own plan once this foundation is validated. Phase 1 deliberately uses a GitHub **Personal Access Token** supplied via config (fastest path to testing storage); the OAuth flow from the spec is added in the dedicated auth-phase plan and slots behind the same `GitHubClient` interface.

---

## File Structure

```
Nimbus/
├── Cargo.toml                      # workspace manifest
├── crates/
│   ├── nimbus-core/                # shared types & errors (no I/O)
│   │   ├── Cargo.toml
│   │   └── src/lib.rs              # DriveFile, FileKind, NimbusError
│   ├── nimbus-github/              # thin GitHub Git Data API client
│   │   ├── Cargo.toml
│   │   └── src/lib.rs              # GitHubClient: get_tree, get_blob, create_blob, commit
│   ├── nimbus-storage/             # maps DriveFile <-> GitHub, uses the cache
│   │   ├── Cargo.toml
│   │   └── src/lib.rs              # StorageEngine: list, upload, download
│   └── nimbus-server/              # Axum app, config, SQLite cache, routes
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs            # bootstrap
│           ├── config.rs          # env-based config + instance secret
│           ├── cache.rs           # SQLite open + migrations
│           └── routes.rs          # HTTP handlers
└── migrations/
    └── 0001_init.sql              # files cache table
```

**Responsibility boundaries:**
- `nimbus-core` — pure data types and the error enum. No network, no DB. Everything else depends on it.
- `nimbus-github` — knows HTTP and GitHub's API shape. Knows nothing about drives or caching.
- `nimbus-storage` — knows the drive model (files, folders) and orchestrates GitHub + cache. Knows nothing about HTTP routing.
- `nimbus-server` — knows HTTP, config, and the SQLite connection. Wires everything together.

---

## Task 1: Workspace scaffold + core types

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/nimbus-core/Cargo.toml`
- Create: `crates/nimbus-core/src/lib.rs`

- [ ] **Step 1: Write the failing test**

In `crates/nimbus-core/src/lib.rs`:

```rust
use serde::{Deserialize, Serialize};

/// Whether a drive entry is a file (blob) or a folder (tree).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    File,
    Folder,
}

/// A single entry in a drive, as Nimbus models it (storage-agnostic).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveFile {
    /// POSIX-style path within the drive, e.g. "docs/notes.md".
    pub path: String,
    pub kind: FileKind,
    /// Size in bytes (0 for folders).
    pub size: u64,
    /// GitHub blob/tree SHA, if known.
    pub sha: Option<String>,
}

impl DriveFile {
    /// File name (last path segment).
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_returns_last_segment() {
        let f = DriveFile {
            path: "docs/sub/notes.md".into(),
            kind: FileKind::File,
            size: 12,
            sha: None,
        };
        assert_eq!(f.name(), "notes.md");
    }

    #[test]
    fn name_handles_root_level_file() {
        let f = DriveFile { path: "readme.txt".into(), kind: FileKind::File, size: 1, sha: None };
        assert_eq!(f.name(), "readme.txt");
    }
}
```

- [ ] **Step 2: Create the manifests**

`Cargo.toml` (workspace root):

```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
anyhow = "1"
thiserror = "2"
```

`crates/nimbus-core/Cargo.toml`:

```toml
[package]
name = "nimbus-core"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { workspace = true }
thiserror = { workspace = true }
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p nimbus-core`
Expected: 2 tests pass.

- [ ] **Step 4: Add the shared error type**

Append to `crates/nimbus-core/src/lib.rs`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum NimbusError {
    #[error("github api error: {0}")]
    GitHub(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("storage error: {0}")]
    Storage(String),
}

pub type Result<T> = std::result::Result<T, NimbusError>;
```

- [ ] **Step 5: Verify it still compiles**

Run: `cargo test -p nimbus-core`
Expected: 2 tests pass, no warnings about unused `NimbusError`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/nimbus-core
git commit -m "feat(core): add workspace + DriveFile/FileKind/NimbusError types"
```

---

## Task 2: GitHub client — encode/decode blob content

The GitHub Git Data API stores blob content as base64. Before doing any network work, lock down the encoding logic with tests (no I/O needed).

**Files:**
- Create: `crates/nimbus-github/Cargo.toml`
- Create: `crates/nimbus-github/src/lib.rs`

- [ ] **Step 1: Write the failing test**

In `crates/nimbus-github/src/lib.rs`:

```rust
use base64::{engine::general_purpose::STANDARD, Engine};

/// Encode raw bytes the way GitHub's create-blob endpoint expects.
pub fn encode_blob(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Decode the base64 content GitHub returns from get-blob.
/// GitHub wraps lines at 60 chars, so whitespace must be stripped first.
pub fn decode_blob(content: &str) -> Result<Vec<u8>, base64::DecodeError> {
    let cleaned: String = content.chars().filter(|c| !c.is_whitespace()).collect();
    STANDARD.decode(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_bytes() {
        let data = b"hello nimbus \xff\x00";
        let encoded = encode_blob(data);
        let decoded = decode_blob(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn decode_tolerates_github_line_wrapping() {
        // GitHub returns base64 with embedded newlines.
        let encoded = encode_blob(b"the quick brown fox jumps over the lazy dog");
        let wrapped = format!("{}\n{}", &encoded[..8], &encoded[8..]);
        let decoded = decode_blob(&wrapped).unwrap();
        assert_eq!(decoded, b"the quick brown fox jumps over the lazy dog");
    }
}
```

- [ ] **Step 2: Create the manifest**

`crates/nimbus-github/Cargo.toml`:

```toml
[package]
name = "nimbus-github"
version = "0.1.0"
edition = "2021"

[dependencies]
nimbus-core = { path = "../nimbus-core" }
serde = { workspace = true }
serde_json = { workspace = true }
reqwest = { version = "0.12", features = ["json"] }
base64 = "0.22"

[dev-dependencies]
tokio = { workspace = true }
wiremock = "0.6"
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p nimbus-github encode`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/nimbus-github
git commit -m "feat(github): base64 blob encode/decode with line-wrap tolerance"
```

---

## Task 3: GitHub client — get a blob over HTTP (mocked)

**Files:**
- Modify: `crates/nimbus-github/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/nimbus-github/src/lib.rs`:

```rust
use serde::Deserialize;

pub struct GitHubClient {
    token: String,
    base_url: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct BlobResponse {
    content: String,
}

impl GitHubClient {
    /// `base_url` is the API root (https://api.github.com in prod;
    /// a mock server URL in tests).
    pub fn new(token: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            base_url: base_url.into(),
            http: reqwest::Client::new(),
        }
    }

    /// Fetch and decode a blob's raw bytes by SHA.
    pub async fn get_blob(&self, owner: &str, repo: &str, sha: &str)
        -> nimbus_core::Result<Vec<u8>>
    {
        let url = format!("{}/repos/{}/{}/git/blobs/{}", self.base_url, owner, repo, sha);
        let resp = self.http.get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", "nimbus")
            .send().await
            .map_err(|e| nimbus_core::NimbusError::GitHub(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(nimbus_core::NimbusError::GitHub(
                format!("get_blob status {}", resp.status())));
        }
        let body: BlobResponse = resp.json().await
            .map_err(|e| nimbus_core::NimbusError::GitHub(e.to_string()))?;
        decode_blob(&body.content)
            .map_err(|e| nimbus_core::NimbusError::GitHub(e.to_string()))
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use serde_json::json;

    #[tokio::test]
    async fn get_blob_decodes_content() {
        let server = MockServer::start().await;
        let encoded = encode_blob(b"file body");
        Mock::given(method("GET"))
            .and(path("/repos/me/drive/git/blobs/abc123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": encoded,
                "encoding": "base64"
            })))
            .mount(&server)
            .await;

        let client = GitHubClient::new("tok", server.uri());
        let bytes = client.get_blob("me", "drive", "abc123").await.unwrap();
        assert_eq!(bytes, b"file body");
    }

    #[tokio::test]
    async fn get_blob_maps_http_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let client = GitHubClient::new("tok", server.uri());
        let err = client.get_blob("me", "drive", "missing").await.unwrap_err();
        assert!(matches!(err, nimbus_core::NimbusError::GitHub(_)));
    }
}
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test -p nimbus-github`
Expected: all tests pass (encode/decode + the 2 HTTP tests).

- [ ] **Step 3: Commit**

```bash
git add crates/nimbus-github
git commit -m "feat(github): get_blob over HTTP with wiremock coverage"
```

---

## Task 4: GitHub client — create a blob (mocked)

**Files:**
- Modify: `crates/nimbus-github/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add a method on `GitHubClient` and a test in `http_tests`:

```rust
// inside impl GitHubClient
/// Create a blob from raw bytes, returning its SHA.
pub async fn create_blob(&self, owner: &str, repo: &str, bytes: &[u8])
    -> nimbus_core::Result<String>
{
    let url = format!("{}/repos/{}/{}/git/blobs", self.base_url, owner, repo);
    let resp = self.http.post(&url)
        .header("Authorization", format!("Bearer {}", self.token))
        .header("User-Agent", "nimbus")
        .json(&serde_json::json!({
            "content": encode_blob(bytes),
            "encoding": "base64"
        }))
        .send().await
        .map_err(|e| nimbus_core::NimbusError::GitHub(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(nimbus_core::NimbusError::GitHub(
            format!("create_blob status {}", resp.status())));
    }
    #[derive(serde::Deserialize)]
    struct ShaResponse { sha: String }
    let body: ShaResponse = resp.json().await
        .map_err(|e| nimbus_core::NimbusError::GitHub(e.to_string()))?;
    Ok(body.sha)
}
```

Test (add to `http_tests`):

```rust
#[tokio::test]
async fn create_blob_returns_sha() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/me/drive/git/blobs"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "sha": "deadbeef",
            "url": "ignored"
        })))
        .mount(&server)
        .await;
    let client = GitHubClient::new("tok", server.uri());
    let sha = client.create_blob("me", "drive", b"new file").await.unwrap();
    assert_eq!(sha, "deadbeef");
}
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test -p nimbus-github create_blob`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/nimbus-github
git commit -m "feat(github): create_blob returns new blob SHA"
```

---

## Task 5: SQLite cache + migration

**Files:**
- Create: `crates/nimbus-server/Cargo.toml`
- Create: `crates/nimbus-server/src/cache.rs`
- Create: `migrations/0001_init.sql`
- Create: `crates/nimbus-server/src/main.rs` (temporary stub so the crate builds)

- [ ] **Step 1: Write the migration**

`migrations/0001_init.sql`:

```sql
CREATE TABLE IF NOT EXISTS cached_files (
    drive    TEXT NOT NULL,            -- "owner/repo"
    path     TEXT NOT NULL,
    kind     TEXT NOT NULL,            -- "file" | "folder"
    size     INTEGER NOT NULL DEFAULT 0,
    sha      TEXT,
    PRIMARY KEY (drive, path)
);
```

- [ ] **Step 2: Write the failing test**

`crates/nimbus-server/src/cache.rs`:

```rust
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

/// Open a pool and run migrations. Use ":memory:" in tests.
pub async fn open(url: &str) -> anyhow::Result<SqlitePool> {
    let pool = SqlitePoolOptions::new().max_connections(5).connect(url).await?;
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
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='cached_files'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(count, 1);
    }
}
```

- [ ] **Step 3: Create the manifest and stub main**

`crates/nimbus-server/Cargo.toml`:

```toml
[package]
name = "nimbus-server"
version = "0.1.0"
edition = "2021"

[dependencies]
nimbus-core = { path = "../nimbus-core" }
nimbus-github = { path = "../nimbus-github" }
axum = "0.7"
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "migrate"] }

[lib]
path = "src/lib.rs"

[[bin]]
name = "nimbus"
path = "src/main.rs"
```

`crates/nimbus-server/src/lib.rs`:

```rust
pub mod cache;
```

`crates/nimbus-server/src/main.rs`:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("nimbus: starting (skeleton)");
    Ok(())
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p nimbus-server migration`
Expected: PASS (table created in-memory).

- [ ] **Step 5: Commit**

```bash
git add crates/nimbus-server migrations
git commit -m "feat(server): SQLite cache pool + initial migration"
```

---

## Task 6: StorageEngine — upload writes a blob and caches it

This is the first cross-module orchestration. `StorageEngine` depends on a `GitHubClient` and a `SqlitePool`.

**Files:**
- Create: `crates/nimbus-storage/Cargo.toml`
- Create: `crates/nimbus-storage/src/lib.rs`

- [ ] **Step 1: Write the failing test**

`crates/nimbus-storage/src/lib.rs`:

```rust
use nimbus_core::{DriveFile, FileKind, Result, NimbusError};
use nimbus_github::GitHubClient;
use sqlx::SqlitePool;

pub struct StorageEngine {
    gh: GitHubClient,
    pool: SqlitePool,
    owner: String,
    repo: String,
}

impl StorageEngine {
    pub fn new(gh: GitHubClient, pool: SqlitePool, owner: impl Into<String>, repo: impl Into<String>) -> Self {
        Self { gh, pool, owner: owner.into(), repo: repo.into() }
    }

    fn drive_key(&self) -> String { format!("{}/{}", self.owner, self.repo) }

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
            "INSERT OR REPLACE INTO cached_files (drive, path, kind, size, sha) VALUES (?, ?, 'file', ?, ?)")
            .bind(self.drive_key())
            .bind(&file.path)
            .bind(file.size as i64)
            .bind(file.sha.as_deref())
            .execute(&self.pool).await
            .map_err(|e| NimbusError::Storage(e.to_string()))?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path as wpath};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use serde_json::json;

    async fn memory_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn upload_creates_blob_and_caches_row() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wpath("/repos/me/drive/git/blobs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha":"sha-1"})))
            .mount(&server).await;

        let gh = GitHubClient::new("tok", server.uri());
        let pool = memory_pool().await;
        let engine = StorageEngine::new(gh, pool.clone(), "me", "drive");

        let file = engine.upload("notes.md", b"hi").await.unwrap();
        assert_eq!(file.sha.as_deref(), Some("sha-1"));
        assert_eq!(file.size, 2);

        let cached: (String, i64) = sqlx::query_as(
            "SELECT sha, size FROM cached_files WHERE drive='me/drive' AND path='notes.md'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(cached.0, "sha-1");
        assert_eq!(cached.1, 2);
    }
}
```

- [ ] **Step 2: Create the manifest**

`crates/nimbus-storage/Cargo.toml`:

```toml
[package]
name = "nimbus-storage"
version = "0.1.0"
edition = "2021"

[dependencies]
nimbus-core = { path = "../nimbus-core" }
nimbus-github = { path = "../nimbus-github" }
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "migrate"] }

[dev-dependencies]
tokio = { workspace = true }
wiremock = "0.6"
serde_json = { workspace = true }
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p nimbus-storage upload`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/nimbus-storage
git commit -m "feat(storage): upload creates GitHub blob and caches metadata"
```

---

## Task 7: StorageEngine — list cached files

**Files:**
- Modify: `crates/nimbus-storage/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add a method and test:

```rust
// inside impl StorageEngine
/// List cached files for this drive, sorted by path.
pub async fn list(&self) -> Result<Vec<DriveFile>> {
    let rows: Vec<(String, String, i64, Option<String>)> = sqlx::query_as(
        "SELECT path, kind, size, sha FROM cached_files WHERE drive = ? ORDER BY path")
        .bind(self.drive_key())
        .fetch_all(&self.pool).await
        .map_err(|e| NimbusError::Storage(e.to_string()))?;
    Ok(rows.into_iter().map(|(path, kind, size, sha)| DriveFile {
        path,
        kind: if kind == "folder" { FileKind::Folder } else { FileKind::File },
        size: size as u64,
        sha,
    }).collect())
}
```

```rust
#[tokio::test]
async fn list_returns_uploaded_files_sorted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha":"s"})))
        .mount(&server).await;
    let gh = GitHubClient::new("tok", server.uri());
    let pool = memory_pool().await;
    let engine = StorageEngine::new(gh, pool, "me", "drive");

    engine.upload("b.txt", b"b").await.unwrap();
    engine.upload("a.txt", b"a").await.unwrap();

    let files = engine.list().await.unwrap();
    let names: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(names, vec!["a.txt", "b.txt"]);
}
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test -p nimbus-storage list`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/nimbus-storage
git commit -m "feat(storage): list cached files sorted by path"
```

---

## Task 8: StorageEngine — download reads a blob by cached SHA

**Files:**
- Modify: `crates/nimbus-storage/src/lib.rs`

- [ ] **Step 1: Write the failing test**

```rust
// inside impl StorageEngine
/// Download a file's bytes: look up its SHA in the cache, then fetch the blob.
pub async fn download(&self, path: &str) -> Result<Vec<u8>> {
    let sha: Option<String> = sqlx::query_scalar(
        "SELECT sha FROM cached_files WHERE drive = ? AND path = ?")
        .bind(self.drive_key())
        .bind(path)
        .fetch_optional(&self.pool).await
        .map_err(|e| NimbusError::Storage(e.to_string()))?
        .flatten();
    let sha = sha.ok_or_else(|| NimbusError::NotFound(path.to_string()))?;
    self.gh.get_blob(&self.owner, &self.repo, &sha).await
}
```

```rust
#[tokio::test]
async fn download_fetches_blob_by_cached_sha() {
    let server = MockServer::start().await;
    let encoded = nimbus_github::encode_blob(b"the body");
    Mock::given(method("POST"))
        .and(wpath("/repos/me/drive/git/blobs"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha":"sha-x"})))
        .mount(&server).await;
    Mock::given(method("GET"))
        .and(wpath("/repos/me/drive/git/blobs/sha-x"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"content": encoded})))
        .mount(&server).await;

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
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test -p nimbus-storage download`
Expected: both tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/nimbus-storage
git commit -m "feat(storage): download blob by cached SHA, NotFound when missing"
```

---

## Task 9: Config from environment

**Files:**
- Create: `crates/nimbus-server/src/config.rs`
- Modify: `crates/nimbus-server/src/lib.rs` (add `pub mod config;`)

- [ ] **Step 1: Write the failing test**

`crates/nimbus-server/src/config.rs`:

```rust
/// Runtime configuration, read from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    pub github_token: String,
    pub drive_owner: String,
    pub drive_repo: String,
    pub database_url: String,
    pub bind_addr: String,
}

impl Config {
    /// Build from a lookup function (real env in prod, a map in tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let req = |k: &str| get(k).ok_or_else(|| anyhow::anyhow!("missing env {k}"));
        Ok(Self {
            github_token: req("NIMBUS_GITHUB_TOKEN")?,
            drive_owner: req("NIMBUS_DRIVE_OWNER")?,
            drive_repo: req("NIMBUS_DRIVE_REPO")?,
            database_url: get("NIMBUS_DATABASE_URL")
                .unwrap_or_else(|| "sqlite:nimbus.db?mode=rwc".into()),
            bind_addr: get("NIMBUS_BIND_ADDR")
                .unwrap_or_else(|| "127.0.0.1:8080".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn map_lookup(m: HashMap<&'static str, &'static str>) -> impl Fn(&str) -> Option<String> {
        move |k| m.get(k).map(|v| v.to_string())
    }

    #[test]
    fn fills_defaults_when_optional_missing() {
        let m = HashMap::from([
            ("NIMBUS_GITHUB_TOKEN", "tok"),
            ("NIMBUS_DRIVE_OWNER", "me"),
            ("NIMBUS_DRIVE_REPO", "drive"),
        ]);
        let cfg = Config::from_lookup(map_lookup(m)).unwrap();
        assert_eq!(cfg.bind_addr, "127.0.0.1:8080");
        assert_eq!(cfg.drive_owner, "me");
    }

    #[test]
    fn errors_when_required_missing() {
        let m = HashMap::new();
        assert!(Config::from_lookup(map_lookup(m)).is_err());
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/nimbus-server/src/lib.rs`:

```rust
pub mod cache;
pub mod config;
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p nimbus-server config`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/nimbus-server
git commit -m "feat(server): env-based Config with testable lookup"
```

---

## Task 10: HTTP routes — list, upload, download

Wire `StorageEngine` into Axum. Test handlers with `axum`'s `oneshot` + a mocked GitHub server, so the full path (HTTP → storage → GitHub → cache) is exercised.

**Files:**
- Create: `crates/nimbus-server/src/routes.rs`
- Modify: `crates/nimbus-server/src/lib.rs` (add `pub mod routes;`)
- Modify: `crates/nimbus-server/Cargo.toml` (add deps below)

- [ ] **Step 1: Add dependencies**

In `crates/nimbus-server/Cargo.toml` add:

```toml
nimbus-storage = { path = "../nimbus-storage" }
tower = { version = "0.5", features = ["util"] }

[dev-dependencies]
wiremock = "0.6"
http-body-util = "0.1"
```

- [ ] **Step 2: Write the failing test**

`crates/nimbus-server/src/routes.rs`:

```rust
use axum::{Router, routing::{get, post}, extract::{State, Path}, Json, http::StatusCode};
use nimbus_storage::StorageEngine;
use nimbus_core::DriveFile;
use std::sync::Arc;

pub type AppState = Arc<StorageEngine>;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/files", get(list_files))
        .route("/api/files/*path", get(download_file).post(upload_file))
        .with_state(state)
}

async fn list_files(State(engine): State<AppState>)
    -> Result<Json<Vec<DriveFile>>, StatusCode>
{
    engine.list().await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn upload_file(State(engine): State<AppState>, Path(path): Path<String>, body: axum::body::Bytes)
    -> Result<Json<DriveFile>, StatusCode>
{
    engine.upload(&path, &body).await
        .map(Json)
        .map_err(|_| StatusCode::BAD_GATEWAY)
}

async fn download_file(State(engine): State<AppState>, Path(path): Path<String>)
    -> Result<axum::body::Bytes, StatusCode>
{
    match engine.download(&path).await {
        Ok(bytes) => Ok(bytes.into()),
        Err(nimbus_core::NimbusError::NotFound(_)) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::BAD_GATEWAY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path as wpath};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use serde_json::json;

    async fn test_engine(gh_uri: String) -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        let gh = nimbus_github::GitHubClient::new("tok", gh_uri);
        Arc::new(StorageEngine::new(gh, pool, "me", "drive"))
    }

    #[tokio::test]
    async fn upload_then_list_roundtrip() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wpath("/repos/me/drive/git/blobs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha":"s1"})))
            .mount(&server).await;

        let app = router(test_engine(server.uri()).await);

        // upload
        let up = app.clone().oneshot(Request::builder()
            .method("POST").uri("/api/files/hello.txt")
            .body(Body::from("hi")).unwrap()).await.unwrap();
        assert_eq!(up.status(), StatusCode::OK);

        // list
        let resp = app.oneshot(Request::builder()
            .uri("/api/files").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let files: Vec<DriveFile> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "hello.txt");
    }

    #[tokio::test]
    async fn download_missing_returns_404() {
        let server = MockServer::start().await;
        let app = router(test_engine(server.uri()).await);
        let resp = app.oneshot(Request::builder()
            .uri("/api/files/nope.txt").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
```

- [ ] **Step 3: Register the module**

In `crates/nimbus-server/src/lib.rs`:

```rust
pub mod cache;
pub mod config;
pub mod routes;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p nimbus-server routes`
Expected: both tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/nimbus-server
git commit -m "feat(server): HTTP routes for list/upload/download with integration tests"
```

---

## Task 11: Wire it all together in main + smoke test

**Files:**
- Modify: `crates/nimbus-server/src/main.rs`
- Create: `README.md`

- [ ] **Step 1: Implement main**

`crates/nimbus-server/src/main.rs`:

```rust
use nimbus_server::{cache, config::Config, routes};
use nimbus_github::GitHubClient;
use nimbus_storage::StorageEngine;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::from_lookup(|k| std::env::var(k).ok())?;
    let pool = cache::open(&cfg.database_url).await?;
    let gh = GitHubClient::new(cfg.github_token.clone(), "https://api.github.com".to_string());
    let engine = Arc::new(StorageEngine::new(gh, pool, cfg.drive_owner.clone(), cfg.drive_repo.clone()));
    let app = routes::router(engine);

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    println!("nimbus listening on http://{}", cfg.bind_addr);
    axum::serve(listener, app).await?;
    Ok(())
}
```

- [ ] **Step 2: Verify the whole workspace builds and all tests pass**

Run: `cargo test`
Expected: all tests across all crates pass.

Run: `cargo build --release`
Expected: builds `target/release/nimbus` with no errors.

- [ ] **Step 3: Write a minimal README**

`README.md`:

```markdown
# Nimbus

Self-hosted, privacy-first cloud drive backed by your own GitHub repositories,
with pluggable AI (bring your own key or local model).

> Status: early development. See `docs/specs/` for the design and `docs/plans/` for the roadmap.

## Run (Phase 1 skeleton)

```bash
export NIMBUS_GITHUB_TOKEN=ghp_xxx       # a token with `repo` scope
export NIMBUS_DRIVE_OWNER=your-username
export NIMBUS_DRIVE_REPO=your-drive-repo
cargo run --release
# -> nimbus listening on http://127.0.0.1:8080
```

## API (Phase 1)

- `GET  /api/files` — list files in the drive
- `POST /api/files/<path>` — upload (request body = raw bytes)
- `GET  /api/files/<path>` — download

## License

MIT
```

- [ ] **Step 4: Commit**

```bash
git add crates/nimbus-server/src/main.rs README.md
git commit -m "feat(server): wire config+cache+github+storage into running binary"
```

---

## Self-Review

**Spec coverage (Phase 1 slice):**
- Spec §3 modules `auth`/`storage`/`index`/`api` → Phase 1 implements `storage` (Tasks 6–8), `index`/cache (Task 5), `api` (Task 10), and a token-based stand-in for `auth` (Task 9 config). OAuth UI, `ai`, `search`, `web` are explicitly deferred to their own phase plans (stated in header). ✅
- Spec §4 GitHub storage model, small-file (<100MB) path → Tasks 2–4, 6–8. Chunking (≥100MB) and `.nimbus/` metadata folder are deferred to the chunking-phase plan. ✅ (called out in header)
- Spec §8 testing (unit + integration with mock GitHub) → every task is TDD with `wiremock`. ✅

**Placeholder scan:** No TBD/TODO; every code step shows complete code; every test step shows the assertion and the exact `cargo test` command. ✅

**Type consistency:** `DriveFile`/`FileKind`/`NimbusError` defined in Task 1 are used unchanged in Tasks 6–10. `GitHubClient::{new,get_blob,create_blob}` defined in Tasks 3–4 match their call sites in Task 6/8. `StorageEngine::{new,upload,list,download}` signatures match between definition and the route handlers in Task 10. `Config` fields used in `main` (Task 11) match Task 9. ✅

**Deferred to subsequent phase plans (not gaps — scoped out):** client-side encryption, large-file chunking + `.nimbus/` manifest, GitHub OAuth flow, AI provider trait + semantic search, file preview, SvelteKit frontend.

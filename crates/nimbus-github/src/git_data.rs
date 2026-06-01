//! Higher-level operations over GitHub's Git database: refs, commits, and trees.
//!
//! A blob created via `create_blob` is unreachable until a commit references it.
//! These methods turn a blob SHA into a durable commit on a branch, and read a
//! branch's file listing back out of its tree.

use crate::GitHubClient;
use nimbus_core::{NimbusError, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// A blob entry discovered while listing a branch's tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeFile {
    pub path: String,
    pub sha: String,
    pub size: u64,
}

/// A single change to apply to a tree: add/update a blob (`Some(sha)`) or
/// delete the entry at `path` (`None`).
#[derive(Debug, Clone)]
pub struct TreeChange {
    pub path: String,
    pub blob_sha: Option<String>,
}

/// One revision of a file, from the commit history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub date: String,
}

#[derive(Deserialize)]
struct RefObject {
    object: ShaHolder,
}

#[derive(Deserialize)]
struct ShaHolder {
    sha: String,
}

#[derive(Deserialize)]
struct CommitResponse {
    tree: ShaHolder,
}

#[derive(Deserialize)]
struct TreeResponse {
    tree: Vec<TreeEntry>,
}

#[derive(Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    sha: String,
    #[serde(default)]
    size: Option<u64>,
}

/// Send a request, fail on non-2xx, and deserialize the JSON body.
async fn json_or_err<T: DeserializeOwned>(req: reqwest::RequestBuilder, ctx: &str) -> Result<T> {
    let resp = req
        .send()
        .await
        .map_err(|e| NimbusError::GitHub(format!("{ctx}: {e}")))?;
    if !resp.status().is_success() {
        return Err(NimbusError::GitHub(format!(
            "{ctx}: status {}",
            resp.status()
        )));
    }
    resp.json()
        .await
        .map_err(|e| NimbusError::GitHub(format!("{ctx}: decode {e}")))
}

impl GitHubClient {
    /// The commit SHA a branch points at, or `None` if the branch doesn't exist (404).
    pub async fn get_branch_head(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Option<String>> {
        let url = format!("{}/git/ref/heads/{}", self.repo_url(owner, repo), branch);
        let resp = self
            .get(&url)
            .send()
            .await
            .map_err(|e| NimbusError::GitHub(format!("get_branch_head: {e}")))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(NimbusError::GitHub(format!(
                "get_branch_head: status {}",
                resp.status()
            )));
        }
        let body: RefObject = resp
            .json()
            .await
            .map_err(|e| NimbusError::GitHub(format!("get_branch_head: decode {e}")))?;
        Ok(Some(body.object.sha))
    }

    /// The tree SHA referenced by a commit.
    pub async fn get_commit_tree(
        &self,
        owner: &str,
        repo: &str,
        commit_sha: &str,
    ) -> Result<String> {
        let url = format!("{}/git/commits/{}", self.repo_url(owner, repo), commit_sha);
        let body: CommitResponse = json_or_err(self.get(&url), "get_commit_tree").await?;
        Ok(body.tree.sha)
    }

    /// Create a tree adding/replacing a single blob at `path`, optionally based on `base_tree`.
    pub async fn create_tree(
        &self,
        owner: &str,
        repo: &str,
        base_tree: Option<&str>,
        path: &str,
        blob_sha: &str,
    ) -> Result<String> {
        let url = format!("{}/git/trees", self.repo_url(owner, repo));
        let mut payload = serde_json::json!({
            "tree": [{ "path": path, "mode": "100644", "type": "blob", "sha": blob_sha }]
        });
        if let Some(base) = base_tree {
            payload["base_tree"] = serde_json::Value::String(base.to_string());
        }
        let body: ShaHolder = json_or_err(self.post(&url).json(&payload), "create_tree").await?;
        Ok(body.sha)
    }

    /// Create a commit pointing at `tree_sha` with the given parents.
    pub async fn create_commit(
        &self,
        owner: &str,
        repo: &str,
        message: &str,
        tree_sha: &str,
        parents: &[String],
    ) -> Result<String> {
        let url = format!("{}/git/commits", self.repo_url(owner, repo));
        let payload = serde_json::json!({
            "message": message,
            "tree": tree_sha,
            "parents": parents,
        });
        let body: ShaHolder = json_or_err(self.post(&url).json(&payload), "create_commit").await?;
        Ok(body.sha)
    }

    /// Move an existing branch ref to `commit_sha`.
    pub async fn update_branch(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        commit_sha: &str,
    ) -> Result<()> {
        let url = format!("{}/git/refs/heads/{}", self.repo_url(owner, repo), branch);
        let payload = serde_json::json!({ "sha": commit_sha, "force": false });
        let _: serde_json::Value =
            json_or_err(self.patch(&url).json(&payload), "update_branch").await?;
        Ok(())
    }

    /// Create a new branch ref pointing at `commit_sha`.
    pub async fn create_branch(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        commit_sha: &str,
    ) -> Result<()> {
        let url = format!("{}/git/refs", self.repo_url(owner, repo));
        let payload =
            serde_json::json!({ "ref": format!("refs/heads/{branch}"), "sha": commit_sha });
        let _: serde_json::Value =
            json_or_err(self.post(&url).json(&payload), "create_branch").await?;
        Ok(())
    }

    /// Commit a single blob to `path` on `branch`, creating the branch if it doesn't exist.
    /// Returns the new commit SHA.
    pub async fn commit_blob(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        path: &str,
        blob_sha: &str,
        message: &str,
    ) -> Result<String> {
        match self.get_branch_head(owner, repo, branch).await? {
            Some(head) => {
                let base_tree = self.get_commit_tree(owner, repo, &head).await?;
                let tree = self
                    .create_tree(owner, repo, Some(&base_tree), path, blob_sha)
                    .await?;
                let commit = self
                    .create_commit(owner, repo, message, &tree, &[head])
                    .await?;
                self.update_branch(owner, repo, branch, &commit).await?;
                Ok(commit)
            }
            None => {
                let tree = self.create_tree(owner, repo, None, path, blob_sha).await?;
                let commit = self.create_commit(owner, repo, message, &tree, &[]).await?;
                self.create_branch(owner, repo, branch, &commit).await?;
                Ok(commit)
            }
        }
    }

    /// Create a tree applying several add/update/delete changes on top of `base_tree`.
    async fn create_tree_multi(
        &self,
        owner: &str,
        repo: &str,
        base_tree: &str,
        changes: &[TreeChange],
    ) -> Result<String> {
        let url = format!("{}/git/trees", self.repo_url(owner, repo));
        let entries: Vec<serde_json::Value> = changes
            .iter()
            .map(|c| match &c.blob_sha {
                Some(sha) => serde_json::json!({
                    "path": c.path, "mode": "100644", "type": "blob", "sha": sha
                }),
                // A null sha removes the entry from the resulting tree.
                None => serde_json::json!({
                    "path": c.path, "mode": "100644", "type": "blob", "sha": serde_json::Value::Null
                }),
            })
            .collect();
        let payload = serde_json::json!({ "base_tree": base_tree, "tree": entries });
        let body: ShaHolder =
            json_or_err(self.post(&url).json(&payload), "create_tree_multi").await?;
        Ok(body.sha)
    }

    /// Apply a batch of tree changes (add/update/delete) as one commit on `branch`.
    /// Returns the new commit SHA. The branch must already exist.
    pub async fn commit_changes(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        changes: &[TreeChange],
        message: &str,
    ) -> Result<String> {
        let head = self
            .get_branch_head(owner, repo, branch)
            .await?
            .ok_or_else(|| NimbusError::GitHub(format!("branch {branch} does not exist")))?;
        let base_tree = self.get_commit_tree(owner, repo, &head).await?;
        let tree = self
            .create_tree_multi(owner, repo, &base_tree, changes)
            .await?;
        let commit = self
            .create_commit(owner, repo, message, &tree, &[head])
            .await?;
        self.update_branch(owner, repo, branch, &commit).await?;
        Ok(commit)
    }

    /// List all blob entries on `branch` (recursive). Empty if the branch is missing.
    pub async fn list_tree(&self, owner: &str, repo: &str, branch: &str) -> Result<Vec<TreeFile>> {
        let head = match self.get_branch_head(owner, repo, branch).await? {
            Some(h) => h,
            None => return Ok(Vec::new()),
        };
        let tree_sha = self.get_commit_tree(owner, repo, &head).await?;
        self.tree_entries(owner, repo, &tree_sha).await
    }

    /// All blob entries of a tree (recursive).
    async fn tree_entries(&self, owner: &str, repo: &str, tree_sha: &str) -> Result<Vec<TreeFile>> {
        let url = format!(
            "{}/git/trees/{}?recursive=1",
            self.repo_url(owner, repo),
            tree_sha
        );
        let body: TreeResponse = json_or_err(self.get(&url), "tree_entries").await?;
        Ok(body
            .tree
            .into_iter()
            .filter(|e| e.kind == "blob")
            .map(|e| TreeFile {
                path: e.path,
                sha: e.sha,
                size: e.size.unwrap_or(0),
            })
            .collect())
    }

    /// The blob entry (sha + size) for `path` as it existed at `commit_sha`.
    pub async fn file_at_commit(
        &self,
        owner: &str,
        repo: &str,
        commit_sha: &str,
        path: &str,
    ) -> Result<Option<TreeFile>> {
        let tree_sha = self.get_commit_tree(owner, repo, commit_sha).await?;
        let entries = self.tree_entries(owner, repo, &tree_sha).await?;
        Ok(entries.into_iter().find(|e| e.path == path))
    }

    /// List the commit history touching `path` on `branch` (newest first).
    pub async fn list_commits(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        path: &str,
    ) -> Result<Vec<CommitInfo>> {
        let url = format!("{}/commits", self.repo_url(owner, repo));
        let req = self
            .get(&url)
            .query(&[("sha", branch), ("path", path), ("per_page", "50")]);
        let items: Vec<CommitListItem> = json_or_err(req, "list_commits").await?;
        Ok(items
            .into_iter()
            .map(|c| CommitInfo {
                sha: c.sha,
                message: c.commit.message,
                date: c.commit.author.date,
            })
            .collect())
    }

    /// List the drive-wide commit history on `branch` (newest first), without
    /// filtering to a single path. Used to build the activity feed. `limit`
    /// caps the page size (GitHub allows up to 100).
    pub async fn list_branch_commits(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        limit: u32,
    ) -> Result<Vec<CommitInfo>> {
        let url = format!("{}/commits", self.repo_url(owner, repo));
        let per_page = limit.clamp(1, 100).to_string();
        let req = self
            .get(&url)
            .query(&[("sha", branch), ("per_page", per_page.as_str())]);
        let items: Vec<CommitListItem> = json_or_err(req, "list_branch_commits").await?;
        Ok(items
            .into_iter()
            .map(|c| CommitInfo {
                sha: c.sha,
                message: c.commit.message,
                date: c.commit.author.date,
            })
            .collect())
    }
}

#[derive(Deserialize)]
struct CommitListItem {
    sha: String,
    commit: CommitMeta,
}

#[derive(Deserialize)]
struct CommitMeta {
    message: String,
    author: CommitAuthor,
}

#[derive(Deserialize)]
struct CommitAuthor {
    date: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn get_branch_head_returns_none_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/me/drive/git/ref/heads/main"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let client = GitHubClient::new("tok", server.uri());
        let head = client.get_branch_head("me", "drive", "main").await.unwrap();
        assert_eq!(head, None);
    }

    #[tokio::test]
    async fn commit_blob_on_existing_branch_walks_full_dance() {
        let server = MockServer::start().await;
        // 1. branch head exists
        Mock::given(method("GET"))
            .and(path("/repos/me/drive/git/ref/heads/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "object": { "sha": "head-commit" }
            })))
            .mount(&server)
            .await;
        // 2. commit -> tree
        Mock::given(method("GET"))
            .and(path("/repos/me/drive/git/commits/head-commit"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "tree": { "sha": "base-tree" }
            })))
            .mount(&server)
            .await;
        // 3. create tree
        Mock::given(method("POST"))
            .and(path("/repos/me/drive/git/trees"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "sha": "new-tree" })))
            .mount(&server)
            .await;
        // 4. create commit
        Mock::given(method("POST"))
            .and(path("/repos/me/drive/git/commits"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "sha": "new-commit" })))
            .mount(&server)
            .await;
        // 5. update ref
        Mock::given(method("PATCH"))
            .and(path("/repos/me/drive/git/refs/heads/main"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "ref": "refs/heads/main" })),
            )
            .mount(&server)
            .await;

        let client = GitHubClient::new("tok", server.uri());
        let commit = client
            .commit_blob("me", "drive", "main", "notes.md", "blob-1", "add notes.md")
            .await
            .unwrap();
        assert_eq!(commit, "new-commit");
    }

    #[tokio::test]
    async fn commit_blob_creates_branch_when_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/me/drive/git/ref/heads/main"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/me/drive/git/trees"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "sha": "t1" })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/me/drive/git/commits"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "sha": "c1" })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/me/drive/git/refs"))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(json!({ "ref": "refs/heads/main" })),
            )
            .mount(&server)
            .await;

        let client = GitHubClient::new("tok", server.uri());
        let commit = client
            .commit_blob("me", "drive", "main", "a.txt", "b1", "init")
            .await
            .unwrap();
        assert_eq!(commit, "c1");
    }

    #[tokio::test]
    async fn list_tree_returns_blobs_only() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/me/drive/git/ref/heads/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "object": { "sha": "h1" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/me/drive/git/commits/h1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "tree": { "sha": "tr1" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/me/drive/git/trees/tr1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "tree": [
                    { "path": "docs", "type": "tree", "sha": "d1" },
                    { "path": "docs/a.md", "type": "blob", "sha": "b1", "size": 10 },
                    { "path": "b.txt", "type": "blob", "sha": "b2", "size": 3 }
                ]
            })))
            .mount(&server)
            .await;

        let client = GitHubClient::new("tok", server.uri());
        let files = client.list_tree("me", "drive", "main").await.unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(
            files[0],
            TreeFile {
                path: "docs/a.md".into(),
                sha: "b1".into(),
                size: 10
            }
        );
        assert_eq!(
            files[1],
            TreeFile {
                path: "b.txt".into(),
                sha: "b2".into(),
                size: 3
            }
        );
    }

    #[tokio::test]
    async fn commit_changes_applies_delete_and_add() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/me/drive/git/ref/heads/main"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "object": { "sha": "h" } })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/me/drive/git/commits/h"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "tree": { "sha": "bt" } })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/me/drive/git/trees"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "sha": "nt" })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/me/drive/git/commits"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "sha": "nc" })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/me/drive/git/refs/heads/main"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "ref": "refs/heads/main" })),
            )
            .mount(&server)
            .await;

        let client = GitHubClient::new("tok", server.uri());
        let changes = vec![
            TreeChange {
                path: "old.txt".into(),
                blob_sha: None,
            },
            TreeChange {
                path: "new.txt".into(),
                blob_sha: Some("b2".into()),
            },
        ];
        let commit = client
            .commit_changes("me", "drive", "main", &changes, "move")
            .await
            .unwrap();
        assert_eq!(commit, "nc");
    }

    #[tokio::test]
    async fn list_commits_parses_history() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/me/drive/commits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "sha": "c2", "commit": { "message": "edit", "author": { "date": "2026-06-01T10:00:00Z" } } },
                { "sha": "c1", "commit": { "message": "create", "author": { "date": "2026-05-31T09:00:00Z" } } }
            ])))
            .mount(&server)
            .await;
        let client = GitHubClient::new("tok", server.uri());
        let commits = client
            .list_commits("me", "drive", "main", "a.txt")
            .await
            .unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha, "c2");
        assert_eq!(commits[0].message, "edit");
    }

    #[tokio::test]
    async fn list_branch_commits_parses_drive_wide_history() {
        use wiremock::matchers::query_param;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/me/drive/commits"))
            .and(query_param("sha", "main"))
            .and(query_param("per_page", "5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "sha": "c3", "commit": { "message": "nimbus: upload a.txt", "author": { "date": "2026-06-02T10:00:00Z" } } },
                { "sha": "c2", "commit": { "message": "nimbus: move a.txt -> b.txt", "author": { "date": "2026-06-01T09:00:00Z" } } }
            ])))
            .mount(&server)
            .await;
        let client = GitHubClient::new("tok", server.uri());
        let commits = client
            .list_branch_commits("me", "drive", "main", 5)
            .await
            .unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha, "c3");
        assert_eq!(commits[0].message, "nimbus: upload a.txt");
        assert_eq!(commits[1].message, "nimbus: move a.txt -> b.txt");
    }

    #[tokio::test]
    async fn list_branch_commits_clamps_limit() {
        use wiremock::matchers::query_param;
        let server = MockServer::start().await;
        // limit 0 must be clamped up to at least 1 (GitHub rejects per_page=0).
        Mock::given(method("GET"))
            .and(path("/repos/me/drive/commits"))
            .and(query_param("per_page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        let client = GitHubClient::new("tok", server.uri());
        let commits = client
            .list_branch_commits("me", "drive", "main", 0)
            .await
            .unwrap();
        assert!(commits.is_empty());
    }

    #[tokio::test]
    async fn list_tree_empty_when_branch_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let client = GitHubClient::new("tok", server.uri());
        let files = client.list_tree("me", "drive", "main").await.unwrap();
        assert!(files.is_empty());
    }
}

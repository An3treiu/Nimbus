//! Higher-level operations over GitHub's Git database: refs, commits, and trees.
//!
//! A blob created via `create_blob` is unreachable until a commit references it.
//! These methods turn a blob SHA into a durable commit on a branch, and read a
//! branch's file listing back out of its tree.

use crate::GitHubClient;
use nimbus_core::{NimbusError, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;

/// A blob entry discovered while listing a branch's tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeFile {
    pub path: String,
    pub sha: String,
    pub size: u64,
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

    /// List all blob entries on `branch` (recursive). Empty if the branch is missing.
    pub async fn list_tree(&self, owner: &str, repo: &str, branch: &str) -> Result<Vec<TreeFile>> {
        let head = match self.get_branch_head(owner, repo, branch).await? {
            Some(h) => h,
            None => return Ok(Vec::new()),
        };
        let tree_sha = self.get_commit_tree(owner, repo, &head).await?;
        let url = format!(
            "{}/git/trees/{}?recursive=1",
            self.repo_url(owner, repo),
            tree_sha
        );
        let body: TreeResponse = json_or_err(self.get(&url), "list_tree").await?;
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

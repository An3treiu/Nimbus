use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;

mod git_data;
mod oauth;

pub use git_data::{CommitInfo, TreeChange, TreeFile};
pub use oauth::{poll_for_token, start_device_flow, DeviceCode, PollResult};

use std::sync::Arc;

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

/// A thin client over GitHub's Git Data API.
///
/// The token is stored in an `ArcSwap` so it can be replaced at runtime (e.g.
/// after an OAuth device-flow login) without rebuilding the client.
pub struct GitHubClient {
    token: arc_swap::ArcSwap<String>,
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
            token: arc_swap::ArcSwap::from_pointee(token.into()),
            base_url: base_url.into(),
            http: reqwest::Client::new(),
        }
    }

    /// Replace the access token at runtime (e.g. after OAuth login).
    pub fn set_token(&self, token: impl Into<String>) {
        self.token.store(Arc::new(token.into()));
    }

    /// The current bearer token value.
    fn bearer(&self) -> String {
        self.token.load().as_str().to_string()
    }

    /// Build a `GET` request with auth + user-agent headers preset.
    pub(crate) fn get(&self, url: &str) -> reqwest::RequestBuilder {
        self.http
            .get(url)
            .header("Authorization", format!("Bearer {}", self.bearer()))
            .header("User-Agent", "nimbus")
    }

    /// Build a `POST` request with auth + user-agent headers preset.
    pub(crate) fn post(&self, url: &str) -> reqwest::RequestBuilder {
        self.http
            .post(url)
            .header("Authorization", format!("Bearer {}", self.bearer()))
            .header("User-Agent", "nimbus")
    }

    /// Build a `PATCH` request with auth + user-agent headers preset.
    pub(crate) fn patch(&self, url: &str) -> reqwest::RequestBuilder {
        self.http
            .patch(url)
            .header("Authorization", format!("Bearer {}", self.bearer()))
            .header("User-Agent", "nimbus")
    }

    /// The API root for a given repo, e.g. `<base>/repos/<owner>/<repo>`.
    pub(crate) fn repo_url(&self, owner: &str, repo: &str) -> String {
        format!("{}/repos/{}/{}", self.base_url, owner, repo)
    }

    /// Return the login of the user the current token authenticates as.
    /// Used to verify an OAuth token actually belongs to the expected account.
    pub async fn get_authenticated_user(&self) -> nimbus_core::Result<String> {
        let url = format!("{}/user", self.base_url);
        let resp = self
            .get(&url)
            .send()
            .await
            .map_err(|e| nimbus_core::NimbusError::GitHub(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(nimbus_core::NimbusError::GitHub(format!(
                "get_authenticated_user status {}",
                resp.status()
            )));
        }
        #[derive(serde::Deserialize)]
        struct User {
            login: String,
        }
        let user: User = resp
            .json()
            .await
            .map_err(|e| nimbus_core::NimbusError::GitHub(e.to_string()))?;
        Ok(user.login)
    }

    /// Fetch and decode a blob's raw bytes by SHA.
    pub async fn get_blob(
        &self,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> nimbus_core::Result<Vec<u8>> {
        let url = format!(
            "{}/repos/{}/{}/git/blobs/{}",
            self.base_url, owner, repo, sha
        );
        let resp = self
            .get(&url)
            .send()
            .await
            .map_err(|e| nimbus_core::NimbusError::GitHub(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(nimbus_core::NimbusError::GitHub(format!(
                "get_blob status {}",
                resp.status()
            )));
        }
        let body: BlobResponse = resp
            .json()
            .await
            .map_err(|e| nimbus_core::NimbusError::GitHub(e.to_string()))?;
        decode_blob(&body.content).map_err(|e| nimbus_core::NimbusError::GitHub(e.to_string()))
    }

    /// Create a blob from raw bytes, returning its SHA.
    pub async fn create_blob(
        &self,
        owner: &str,
        repo: &str,
        bytes: &[u8],
    ) -> nimbus_core::Result<String> {
        let url = format!("{}/repos/{}/{}/git/blobs", self.base_url, owner, repo);
        let resp = self
            .post(&url)
            .json(&serde_json::json!({
                "content": encode_blob(bytes),
                "encoding": "base64"
            }))
            .send()
            .await
            .map_err(|e| nimbus_core::NimbusError::GitHub(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(nimbus_core::NimbusError::GitHub(format!(
                "create_blob status {}",
                resp.status()
            )));
        }
        #[derive(Deserialize)]
        struct ShaResponse {
            sha: String,
        }
        let body: ShaResponse = resp
            .json()
            .await
            .map_err(|e| nimbus_core::NimbusError::GitHub(e.to_string()))?;
        Ok(body.sha)
    }
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

#[cfg(test)]
mod http_tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
        let sha = client
            .create_blob("me", "drive", b"new file")
            .await
            .unwrap();
        assert_eq!(sha, "deadbeef");
    }
}

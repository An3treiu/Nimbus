//! GitHub OAuth **device flow** — the right fit for a self-hosted app, since it
//! needs no callback URL or client secret. The user is shown a short code to
//! enter at github.com; we poll until they authorize, then receive a token.

use nimbus_core::{NimbusError, Result};
use serde::{Deserialize, Serialize};

/// The device/user codes returned when starting the flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default)]
    pub expires_in: u64,
}

fn default_interval() -> u64 {
    5
}

/// Outcome of polling the token endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollResult {
    Authorized(String),
    Pending,
    SlowDown,
    Denied,
    Failed(String),
}

/// Begin the device flow. `base_url` is normally `https://github.com`.
pub async fn start_device_flow(base_url: &str, client_id: &str, scope: &str) -> Result<DeviceCode> {
    let url = format!("{base_url}/login/device/code");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("Accept", "application/json")
        .json(&serde_json::json!({ "client_id": client_id, "scope": scope }))
        .send()
        .await
        .map_err(|e| NimbusError::GitHub(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(NimbusError::GitHub(format!(
            "device code request status {}",
            resp.status()
        )));
    }
    resp.json::<DeviceCode>()
        .await
        .map_err(|e| NimbusError::GitHub(e.to_string()))
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error: Option<String>,
}

/// Poll once for the access token using the device code.
pub async fn poll_for_token(
    base_url: &str,
    client_id: &str,
    device_code: &str,
) -> Result<PollResult> {
    let url = format!("{base_url}/login/oauth/access_token");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "client_id": client_id,
            "device_code": device_code,
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code"
        }))
        .send()
        .await
        .map_err(|e| NimbusError::GitHub(e.to_string()))?;
    let body: TokenResponse = resp
        .json()
        .await
        .map_err(|e| NimbusError::GitHub(e.to_string()))?;

    if let Some(token) = body.access_token {
        return Ok(PollResult::Authorized(token));
    }
    Ok(match body.error.as_deref() {
        Some("authorization_pending") => PollResult::Pending,
        Some("slow_down") => PollResult::SlowDown,
        Some("access_denied") => PollResult::Denied,
        Some(other) => PollResult::Failed(other.to_string()),
        None => PollResult::Failed("unknown response".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn start_device_flow_parses_codes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login/device/code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "device_code": "dev123",
                "user_code": "WXYZ-1234",
                "verification_uri": "https://github.com/login/device",
                "expires_in": 900,
                "interval": 5
            })))
            .mount(&server)
            .await;
        let dc = start_device_flow(&server.uri(), "client", "repo")
            .await
            .unwrap();
        assert_eq!(dc.user_code, "WXYZ-1234");
        assert_eq!(dc.device_code, "dev123");
        assert_eq!(dc.interval, 5);
    }

    #[tokio::test]
    async fn poll_returns_pending_then_authorized() {
        let server = MockServer::start().await;
        // First mount pending, then we replace by mounting authorized with priority.
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "error": "authorization_pending"
            })))
            .mount(&server)
            .await;
        let r = poll_for_token(&server.uri(), "client", "dev123")
            .await
            .unwrap();
        assert_eq!(r, PollResult::Pending);

        let server2 = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "gho_token",
                "token_type": "bearer",
                "scope": "repo"
            })))
            .mount(&server2)
            .await;
        let r2 = poll_for_token(&server2.uri(), "client", "dev123")
            .await
            .unwrap();
        assert_eq!(r2, PollResult::Authorized("gho_token".into()));
    }

    #[tokio::test]
    async fn poll_maps_access_denied() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "error": "access_denied" })),
            )
            .mount(&server)
            .await;
        let r = poll_for_token(&server.uri(), "client", "dev123")
            .await
            .unwrap();
        assert_eq!(r, PollResult::Denied);
    }
}

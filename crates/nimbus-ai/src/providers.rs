//! Concrete [`AiProvider`] implementations.

use crate::{AiError, AiProvider, ChatProvider, Embedding};
use async_trait::async_trait;
use serde::Deserialize;

/// OpenAI-compatible embeddings provider.
///
/// Works with OpenAI, Google's OpenAI-compatible endpoint, LM Studio,
/// llama.cpp's server, and anything else that exposes `POST {base}/embeddings`.
pub struct OpenAiProvider {
    base_url: String,
    api_key: String,
    model: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    data: Vec<OpenAiDatum>,
}

#[derive(Deserialize)]
struct OpenAiDatum {
    embedding: Embedding,
}

impl OpenAiProvider {
    /// `base_url` should include the API version, e.g. `https://api.openai.com/v1`.
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl AiProvider for OpenAiProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>, AiError> {
        let url = format!("{}/embeddings", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&serde_json::json!({ "model": self.model, "input": texts }))
            .send()
            .await
            .map_err(|e| AiError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AiError::Http(format!("status {}", resp.status())));
        }
        let body: OpenAiResponse = resp
            .json()
            .await
            .map_err(|e| AiError::Decode(e.to_string()))?;
        if body.data.is_empty() {
            return Err(AiError::Empty);
        }
        Ok(body.data.into_iter().map(|d| d.embedding).collect())
    }
}

/// Native Ollama embeddings provider (fully local; nothing leaves the machine).
pub struct OllamaProvider {
    base_url: String,
    model: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct OllamaResponse {
    embeddings: Vec<Embedding>,
}

impl OllamaProvider {
    /// `base_url` is the Ollama root, e.g. `http://localhost:11434`.
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl AiProvider for OllamaProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>, AiError> {
        let url = format!("{}/api/embed", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&serde_json::json!({ "model": self.model, "input": texts }))
            .send()
            .await
            .map_err(|e| AiError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AiError::Http(format!("status {}", resp.status())));
        }
        let body: OllamaResponse = resp
            .json()
            .await
            .map_err(|e| AiError::Decode(e.to_string()))?;
        if body.embeddings.is_empty() {
            return Err(AiError::Empty);
        }
        Ok(body.embeddings)
    }
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

#[derive(Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: ChatMessage,
}

#[async_trait]
impl ChatProvider for OpenAiProvider {
    async fn chat(&self, system: &str, user: &str) -> Result<String, AiError> {
        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&serde_json::json!({
                "model": self.model,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": user }
                ]
            }))
            .send()
            .await
            .map_err(|e| AiError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AiError::Http(format!("status {}", resp.status())));
        }
        let body: OpenAiChatResponse = resp
            .json()
            .await
            .map_err(|e| AiError::Decode(e.to_string()))?;
        body.choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or(AiError::Empty)
    }
}

#[derive(Deserialize)]
struct OllamaChatResponse {
    message: ChatMessage,
}

#[async_trait]
impl ChatProvider for OllamaProvider {
    async fn chat(&self, system: &str, user: &str) -> Result<String, AiError> {
        let url = format!("{}/api/chat", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&serde_json::json!({
                "model": self.model,
                "stream": false,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": user }
                ]
            }))
            .send()
            .await
            .map_err(|e| AiError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AiError::Http(format!("status {}", resp.status())));
        }
        let body: OllamaChatResponse = resp
            .json()
            .await
            .map_err(|e| AiError::Decode(e.to_string()))?;
        Ok(body.message.content)
    }
}

/// Anthropic (Claude) chat provider. Chat-only — Anthropic has no embeddings
/// API, so use OpenAI or Ollama for semantic search.
pub struct AnthropicProvider {
    base_url: String,
    api_key: String,
    model: String,
    http: reqwest::Client,
}

impl AnthropicProvider {
    /// `base_url` is the API root, e.g. `https://api.anthropic.com`.
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            http: reqwest::Client::new(),
        }
    }
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicBlock>,
}

#[derive(Deserialize)]
struct AnthropicBlock {
    #[serde(default)]
    text: String,
}

#[async_trait]
impl ChatProvider for AnthropicProvider {
    async fn chat(&self, system: &str, user: &str) -> Result<String, AiError> {
        let url = format!("{}/v1/messages", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&serde_json::json!({
                "model": self.model,
                "max_tokens": 1024,
                "system": system,
                "messages": [ { "role": "user", "content": user } ]
            }))
            .send()
            .await
            .map_err(|e| AiError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AiError::Http(format!("status {}", resp.status())));
        }
        let body: AnthropicResponse = resp
            .json()
            .await
            .map_err(|e| AiError::Decode(e.to_string()))?;
        body.content
            .into_iter()
            .next()
            .map(|b| b.text)
            .ok_or(AiError::Empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn openai_provider_parses_embeddings() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    { "embedding": [0.1, 0.2, 0.3] },
                    { "embedding": [0.4, 0.5, 0.6] }
                ]
            })))
            .mount(&server)
            .await;

        let p = OpenAiProvider::new(
            format!("{}/v1", server.uri()),
            "key",
            "text-embedding-3-small",
        );
        let out = p.embed(&["a".into(), "b".into()]).await.unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], vec![0.1, 0.2, 0.3]);
    }

    #[tokio::test]
    async fn openai_provider_maps_http_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let p = OpenAiProvider::new(format!("{}/v1", server.uri()), "bad", "m");
        assert!(matches!(
            p.embed(&["x".into()]).await,
            Err(AiError::Http(_))
        ));
    }

    #[tokio::test]
    async fn ollama_provider_parses_embeddings() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "embeddings": [[1.0, 0.0], [0.0, 1.0]]
            })))
            .mount(&server)
            .await;
        let p = OllamaProvider::new(server.uri(), "nomic-embed-text");
        let out = p.embed(&["a".into(), "b".into()]).await.unwrap();
        assert_eq!(out, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    }

    #[tokio::test]
    async fn openai_provider_chat_returns_reply() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [ { "message": { "role": "assistant", "content": "the answer" } } ]
            })))
            .mount(&server)
            .await;
        let p = OpenAiProvider::new(format!("{}/v1", server.uri()), "key", "gpt-4o-mini");
        let reply = p.chat("be helpful", "hi").await.unwrap();
        assert_eq!(reply, "the answer");
    }

    #[tokio::test]
    async fn ollama_provider_chat_returns_reply() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "message": { "role": "assistant", "content": "local reply" }
            })))
            .mount(&server)
            .await;
        let p = OllamaProvider::new(server.uri(), "llama3");
        let reply = p.chat("sys", "q").await.unwrap();
        assert_eq!(reply, "local reply");
    }

    #[tokio::test]
    async fn anthropic_provider_chat_returns_reply() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [ { "type": "text", "text": "claude says hi" } ]
            })))
            .mount(&server)
            .await;
        let p = AnthropicProvider::new(server.uri(), "sk-ant", "claude-opus-4-8");
        let reply = p.chat("system", "hello").await.unwrap();
        assert_eq!(reply, "claude says hi");
    }
}

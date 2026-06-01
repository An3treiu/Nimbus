/// Runtime configuration, read from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    /// GitHub token from env (a PAT). Optional — OAuth device flow can provide one.
    pub github_token: Option<String>,
    /// OAuth App client id, enabling the in-app "Connect GitHub" device flow.
    pub github_client_id: Option<String>,
    pub drive_owner: String,
    pub drive_repo: String,
    pub drive_branch: String,
    pub database_url: String,
    pub bind_addr: String,
    /// When set, files are client-side encrypted. None disables encryption.
    pub encryption_passphrase: Option<String>,
    /// Optional recovery key used to unlock the vault if the passphrase is lost.
    pub recovery_key: Option<String>,
    /// Optional file path to write the one-time recovery key to (instead of stdout).
    pub recovery_key_out: Option<String>,
    /// AI provider for semantic search: "openai", "ollama", or None to disable.
    pub ai_provider: Option<String>,
    pub ai_base_url: Option<String>,
    pub ai_api_key: Option<String>,
    pub ai_model: Option<String>,
    /// Chat model (for "chat with your files"); defaults per provider.
    pub ai_chat_model: Option<String>,
    /// Optional directory of static frontend assets to serve from disk.
    /// When unset, the frontend embedded in the binary is served.
    pub web_dir: Option<String>,
}

impl Config {
    /// Build from a lookup function (real env in prod, a map in tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let req = |k: &str| get(k).ok_or_else(|| anyhow::anyhow!("missing env {k}"));
        Ok(Self {
            github_token: get("NIMBUS_GITHUB_TOKEN").filter(|s| !s.is_empty()),
            github_client_id: get("NIMBUS_GITHUB_CLIENT_ID").filter(|s| !s.is_empty()),
            drive_owner: req("NIMBUS_DRIVE_OWNER")?,
            drive_repo: req("NIMBUS_DRIVE_REPO")?,
            drive_branch: get("NIMBUS_DRIVE_BRANCH").unwrap_or_else(|| "main".into()),
            database_url: get("NIMBUS_DATABASE_URL")
                .unwrap_or_else(|| "sqlite:nimbus.db?mode=rwc".into()),
            bind_addr: get("NIMBUS_BIND_ADDR").unwrap_or_else(|| "127.0.0.1:8080".into()),
            encryption_passphrase: get("NIMBUS_ENCRYPTION_PASSPHRASE").filter(|s| !s.is_empty()),
            recovery_key: get("NIMBUS_RECOVERY_KEY").filter(|s| !s.is_empty()),
            recovery_key_out: get("NIMBUS_RECOVERY_KEY_OUT").filter(|s| !s.is_empty()),
            ai_provider: get("NIMBUS_AI_PROVIDER").filter(|s| !s.is_empty()),
            ai_base_url: get("NIMBUS_AI_BASE_URL").filter(|s| !s.is_empty()),
            ai_api_key: get("NIMBUS_AI_API_KEY").filter(|s| !s.is_empty()),
            ai_model: get("NIMBUS_AI_MODEL").filter(|s| !s.is_empty()),
            ai_chat_model: get("NIMBUS_AI_CHAT_MODEL").filter(|s| !s.is_empty()),
            web_dir: get("NIMBUS_WEB_DIR").filter(|s| !s.is_empty()),
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

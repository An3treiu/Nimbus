/// Runtime configuration, read from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    pub github_token: String,
    pub drive_owner: String,
    pub drive_repo: String,
    pub drive_branch: String,
    pub database_url: String,
    pub bind_addr: String,
    /// When set, files are client-side encrypted. None disables encryption.
    pub encryption_passphrase: Option<String>,
}

impl Config {
    /// Build from a lookup function (real env in prod, a map in tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let req = |k: &str| get(k).ok_or_else(|| anyhow::anyhow!("missing env {k}"));
        Ok(Self {
            github_token: req("NIMBUS_GITHUB_TOKEN")?,
            drive_owner: req("NIMBUS_DRIVE_OWNER")?,
            drive_repo: req("NIMBUS_DRIVE_REPO")?,
            drive_branch: get("NIMBUS_DRIVE_BRANCH").unwrap_or_else(|| "main".into()),
            database_url: get("NIMBUS_DATABASE_URL")
                .unwrap_or_else(|| "sqlite:nimbus.db?mode=rwc".into()),
            bind_addr: get("NIMBUS_BIND_ADDR").unwrap_or_else(|| "127.0.0.1:8080".into()),
            encryption_passphrase: get("NIMBUS_ENCRYPTION_PASSPHRASE")
                .filter(|s| !s.is_empty()),
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

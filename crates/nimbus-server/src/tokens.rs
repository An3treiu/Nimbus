//! Persistence for the OAuth-obtained GitHub access token.
//!
//! The token is a credential, so it is **encrypted at rest** with the instance
//! [`Vault`] (the same key protecting file contents). If encryption is disabled
//! (no vault) we **fail closed**: the token is kept in memory only and never
//! written as plaintext.

use base64::{engine::general_purpose::STANDARD, Engine};
use nimbus_crypto::Vault;
use sqlx::SqlitePool;

/// Associated data binding the ciphertext to its purpose.
const TOKEN_AAD: &[u8] = b"github_token";

/// Load and decrypt the stored GitHub token, if any.
pub async fn load_token(
    pool: &SqlitePool,
    vault: Option<&Vault>,
) -> anyhow::Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT token FROM github_token WHERE id = 1")
        .fetch_optional(pool)
        .await?;
    let Some((encoded,)) = row else {
        return Ok(None);
    };
    let Some(vault) = vault else {
        eprintln!("nimbus: a stored OAuth token exists but encryption is disabled — ignoring it");
        return Ok(None);
    };
    let ciphertext = STANDARD.decode(encoded)?;
    let plaintext = vault
        .open(TOKEN_AAD, &ciphertext)
        .map_err(|e| anyhow::anyhow!("failed to decrypt stored token: {e}"))?;
    Ok(Some(String::from_utf8(plaintext)?))
}

/// Encrypt and persist the GitHub token. Without a vault, refuses to persist
/// (returns `false`) so the token is never written in plaintext.
pub async fn save_token(
    pool: &SqlitePool,
    vault: Option<&Vault>,
    token: &str,
) -> anyhow::Result<bool> {
    let Some(vault) = vault else {
        eprintln!("nimbus: encryption disabled — OAuth token kept in memory only (not persisted)");
        return Ok(false);
    };
    let ciphertext = vault
        .seal(TOKEN_AAD, token.as_bytes())
        .map_err(|e| anyhow::anyhow!("failed to encrypt token: {e}"))?;
    let encoded = STANDARD.encode(ciphertext);
    sqlx::query("INSERT OR REPLACE INTO github_token (id, token) VALUES (1, ?)")
        .bind(encoded)
        .execute(pool)
        .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn memory_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn encrypted_save_then_load_round_trips() {
        let pool = memory_pool().await;
        let vault = Vault::new(nimbus_crypto::generate_key());

        assert_eq!(load_token(&pool, Some(&vault)).await.unwrap(), None);
        assert!(save_token(&pool, Some(&vault), "gho_abc").await.unwrap());
        assert_eq!(
            load_token(&pool, Some(&vault)).await.unwrap(),
            Some("gho_abc".into())
        );
    }

    #[tokio::test]
    async fn stored_token_is_not_plaintext() {
        let pool = memory_pool().await;
        let vault = Vault::new(nimbus_crypto::generate_key());
        save_token(&pool, Some(&vault), "gho_secret").await.unwrap();

        let (raw,): (String,) = sqlx::query_as("SELECT token FROM github_token WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            !raw.contains("gho_secret"),
            "token must not be stored in plaintext"
        );
    }

    #[tokio::test]
    async fn without_vault_does_not_persist() {
        let pool = memory_pool().await;
        assert!(!save_token(&pool, None, "gho_abc").await.unwrap());
        // Nothing was written.
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM github_token")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn wrong_vault_cannot_decrypt() {
        let pool = memory_pool().await;
        let vault = Vault::new(nimbus_crypto::generate_key());
        save_token(&pool, Some(&vault), "gho_abc").await.unwrap();

        let other = Vault::new(nimbus_crypto::generate_key());
        assert!(load_token(&pool, Some(&other)).await.is_err());
    }
}

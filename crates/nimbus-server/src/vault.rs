//! First-run setup and unlocking of the encryption vault.
//!
//! On first run we generate a data key (DEK), wrap it under both the
//! passphrase-derived key and a fresh recovery key, and persist the wrapped
//! forms. On later runs we re-derive the passphrase key and unwrap the DEK.
//! The DEK is held only in memory, inside the returned [`Vault`].

use nimbus_crypto::{
    derive_key, encode_recovery_key, generate_key, generate_salt, unwrap_key, wrap_key, Vault,
};
use sqlx::SqlitePool;

/// Result of unlocking the vault. `new_recovery_key` is `Some` only on first run.
pub struct VaultSetup {
    pub vault: Vault,
    pub new_recovery_key: Option<String>,
}

/// Unlock the existing vault with `passphrase`, or initialize one on first run.
pub async fn unlock_or_init(pool: &SqlitePool, passphrase: &str) -> anyhow::Result<VaultSetup> {
    let existing: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT salt, wrapped_dek, wrapped_dek_recovery FROM vault WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?;

    if let Some((salt, wrapped_dek, _wrapped_rec)) = existing {
        let kek = derive_key(passphrase, &salt)
            .map_err(|e| anyhow::anyhow!("key derivation failed: {e}"))?;
        let dek = unwrap_key(&kek, &wrapped_dek)
            .map_err(|_| anyhow::anyhow!("wrong encryption passphrase"))?;
        return Ok(VaultSetup {
            vault: Vault::new(dek),
            new_recovery_key: None,
        });
    }

    // First run: generate everything and persist the wrapped DEK.
    let salt = generate_salt();
    let dek = generate_key();
    let recovery = generate_key();
    let kek = derive_key(passphrase, &salt).map_err(|e| anyhow::anyhow!("key derivation failed: {e}"))?;
    let wrapped_dek = wrap_key(&kek, &dek).map_err(|e| anyhow::anyhow!("wrap failed: {e}"))?;
    let wrapped_rec = wrap_key(&recovery, &dek).map_err(|e| anyhow::anyhow!("wrap failed: {e}"))?;

    sqlx::query("INSERT INTO vault (id, salt, wrapped_dek, wrapped_dek_recovery) VALUES (1, ?, ?, ?)")
        .bind(salt.as_slice())
        .bind(wrapped_dek.as_slice())
        .bind(wrapped_rec.as_slice())
        .execute(pool)
        .await?;

    Ok(VaultSetup {
        vault: Vault::new(dek),
        new_recovery_key: Some(encode_recovery_key(&recovery)),
    })
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
    async fn first_run_returns_recovery_key_then_unlocks_same_dek() {
        let pool = memory_pool().await;

        let first = unlock_or_init(&pool, "hunter2").await.unwrap();
        assert!(first.new_recovery_key.is_some(), "first run yields a recovery key");
        let sealed = first.vault.seal(b"data").unwrap();

        // Re-unlock with the same passphrase: no new recovery key, same DEK.
        let again = unlock_or_init(&pool, "hunter2").await.unwrap();
        assert!(again.new_recovery_key.is_none());
        assert_eq!(again.vault.open(&sealed).unwrap(), b"data");
    }

    #[tokio::test]
    async fn wrong_passphrase_is_rejected() {
        let pool = memory_pool().await;
        unlock_or_init(&pool, "correct").await.unwrap();
        let result = unlock_or_init(&pool, "wrong").await;
        assert!(result.is_err());
        let msg = result.err().unwrap().to_string();
        assert!(msg.contains("wrong encryption passphrase"), "got: {msg}");
    }
}

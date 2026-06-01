//! Public share links: a random token maps to a file path, optionally guarded
//! by a password (Argon2id) and/or an expiry time.
//!
//! Zero-knowledge note: Nimbus encrypts a drive under a single data key, so a
//! shared file is decrypted server-side before being served. True per-file
//! zero-knowledge sharing (key-in-URL-fragment) requires per-file keys and is a
//! future enhancement; until then, only share from instances you trust.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use nimbus_crypto::{derive_key, generate_key, generate_salt};
use sqlx::SqlitePool;

#[derive(Debug, PartialEq, Eq)]
pub enum ShareError {
    NotFound,
    Expired,
    PasswordRequired,
    BadPassword,
    Db(String),
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Create a share for `path` and return its token.
pub async fn create_share(
    pool: &SqlitePool,
    drive: &str,
    path: &str,
    password: Option<&str>,
    expires_at: Option<i64>,
) -> anyhow::Result<String> {
    let token = URL_SAFE_NO_PAD.encode(generate_key());
    let (salt, hash): (Option<Vec<u8>>, Option<Vec<u8>>) = match password {
        Some(pw) if !pw.is_empty() => {
            let salt = generate_salt();
            let hash = derive_key(pw, &salt).map_err(|e| anyhow::anyhow!("hash: {e}"))?;
            (Some(salt.to_vec()), Some(hash.to_vec()))
        }
        _ => (None, None),
    };
    sqlx::query(
        "INSERT INTO shares (token, drive, path, pw_salt, pw_hash, expires_at, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&token)
    .bind(drive)
    .bind(path)
    .bind(salt)
    .bind(hash)
    .bind(expires_at)
    .bind(now_secs())
    .execute(pool)
    .await?;
    Ok(token)
}

/// Resolve a share token (checking expiry + password), returning the file path.
pub async fn resolve_share(
    pool: &SqlitePool,
    token: &str,
    password: Option<&str>,
) -> Result<String, ShareError> {
    let row: Option<(String, Option<Vec<u8>>, Option<Vec<u8>>, Option<i64>)> =
        sqlx::query_as("SELECT path, pw_salt, pw_hash, expires_at FROM shares WHERE token = ?")
            .bind(token)
            .fetch_optional(pool)
            .await
            .map_err(|e| ShareError::Db(e.to_string()))?;

    let (path, pw_salt, pw_hash, expires_at) = row.ok_or(ShareError::NotFound)?;

    if let Some(exp) = expires_at {
        if now_secs() > exp {
            return Err(ShareError::Expired);
        }
    }
    if let (Some(salt), Some(expected)) = (pw_salt, pw_hash) {
        let provided = password.ok_or(ShareError::PasswordRequired)?;
        let got = derive_key(provided, &salt).map_err(|_| ShareError::BadPassword)?;
        if got.as_slice() != expected.as_slice() {
            return Err(ShareError::BadPassword);
        }
    }
    Ok(path)
}

/// Delete a share by token.
pub async fn revoke_share(pool: &SqlitePool, token: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM shares WHERE token = ?")
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
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
    async fn create_and_resolve_no_password() {
        let pool = memory_pool().await;
        let token = create_share(&pool, "me/d", "a.txt", None, None)
            .await
            .unwrap();
        assert_eq!(resolve_share(&pool, &token, None).await.unwrap(), "a.txt");
    }

    #[tokio::test]
    async fn unknown_token_is_not_found() {
        let pool = memory_pool().await;
        assert_eq!(
            resolve_share(&pool, "nope", None).await.unwrap_err(),
            ShareError::NotFound
        );
    }

    #[tokio::test]
    async fn expired_share_is_rejected() {
        let pool = memory_pool().await;
        let token = create_share(&pool, "me/d", "a.txt", None, Some(now_secs() - 10))
            .await
            .unwrap();
        assert_eq!(
            resolve_share(&pool, &token, None).await.unwrap_err(),
            ShareError::Expired
        );
    }

    #[tokio::test]
    async fn password_is_enforced() {
        let pool = memory_pool().await;
        let token = create_share(&pool, "me/d", "a.txt", Some("hunter2"), None)
            .await
            .unwrap();
        assert_eq!(
            resolve_share(&pool, &token, None).await.unwrap_err(),
            ShareError::PasswordRequired
        );
        assert_eq!(
            resolve_share(&pool, &token, Some("wrong"))
                .await
                .unwrap_err(),
            ShareError::BadPassword
        );
        assert_eq!(
            resolve_share(&pool, &token, Some("hunter2")).await.unwrap(),
            "a.txt"
        );
    }

    #[tokio::test]
    async fn revoke_removes_share() {
        let pool = memory_pool().await;
        let token = create_share(&pool, "me/d", "a.txt", None, None)
            .await
            .unwrap();
        revoke_share(&pool, &token).await.unwrap();
        assert_eq!(
            resolve_share(&pool, &token, None).await.unwrap_err(),
            ShareError::NotFound
        );
    }
}

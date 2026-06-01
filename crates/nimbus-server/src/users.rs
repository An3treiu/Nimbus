//! Multi-user accounts and sessions.
//!
//! Passwords are hashed with Argon2id (per-user salt). Login issues a random
//! session token stored server-side; the browser holds it in an HttpOnly cookie.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use nimbus_crypto::{derive_key, generate_key, generate_salt};
use sqlx::SqlitePool;

/// Default session lifetime: 30 days.
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 3600;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// How many accounts exist.
pub async fn user_count(pool: &SqlitePool) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?)
}

/// Create a user. Errors if the username already exists.
pub async fn create_user(pool: &SqlitePool, username: &str, password: &str) -> anyhow::Result<()> {
    if username.is_empty() || password.is_empty() {
        anyhow::bail!("username and password are required");
    }
    let salt = generate_salt();
    let hash = derive_key(password, &salt).map_err(|e| anyhow::anyhow!("hash: {e}"))?;
    sqlx::query("INSERT INTO users (username, pw_salt, pw_hash, created_at) VALUES (?, ?, ?, ?)")
        .bind(username)
        .bind(salt.as_slice())
        .bind(hash.as_slice())
        .bind(now_secs())
        .execute(pool)
        .await
        .map_err(|e| anyhow::anyhow!("create_user: {e}"))?;
    Ok(())
}

/// Atomically create the first account: inserts only if the users table is
/// empty, eliminating a TOCTOU race between concurrent registrations. Returns
/// `true` if this call created the account.
pub async fn register_first(
    pool: &SqlitePool,
    username: &str,
    password: &str,
) -> anyhow::Result<bool> {
    if username.is_empty() || password.is_empty() {
        anyhow::bail!("username and password are required");
    }
    let salt = generate_salt();
    let hash = derive_key(password, &salt).map_err(|e| anyhow::anyhow!("hash: {e}"))?;
    let res = sqlx::query(
        "INSERT INTO users (username, pw_salt, pw_hash, created_at) \
         SELECT ?, ?, ?, ? WHERE NOT EXISTS (SELECT 1 FROM users)",
    )
    .bind(username)
    .bind(salt.as_slice())
    .bind(hash.as_slice())
    .bind(now_secs())
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Verify a username/password pair.
pub async fn verify_login(
    pool: &SqlitePool,
    username: &str,
    password: &str,
) -> anyhow::Result<bool> {
    let row: Option<(Vec<u8>, Vec<u8>)> =
        sqlx::query_as("SELECT pw_salt, pw_hash FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(pool)
            .await?;
    let Some((salt, expected)) = row else {
        return Ok(false);
    };
    let got = derive_key(password, &salt).map_err(|e| anyhow::anyhow!("hash: {e}"))?;
    Ok(nimbus_crypto::constant_eq(got.as_slice(), &expected))
}

/// Create a session for `username` and return its token.
pub async fn create_session(pool: &SqlitePool, username: &str) -> anyhow::Result<String> {
    let token = URL_SAFE_NO_PAD.encode(generate_key());
    sqlx::query("INSERT INTO sessions (token, username, expires_at) VALUES (?, ?, ?)")
        .bind(&token)
        .bind(username)
        .bind(now_secs() + SESSION_TTL_SECS)
        .execute(pool)
        .await?;
    Ok(token)
}

/// Return the username for a valid, unexpired session token.
pub async fn validate_session(pool: &SqlitePool, token: &str) -> anyhow::Result<Option<String>> {
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT username, expires_at FROM sessions WHERE token = ?")
            .bind(token)
            .fetch_optional(pool)
            .await?;
    match row {
        Some((username, exp)) if exp > now_secs() => Ok(Some(username)),
        _ => Ok(None),
    }
}

/// Delete a session (logout).
pub async fn delete_session(pool: &SqlitePool, token: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token = ?")
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
    async fn create_and_verify_login() {
        let pool = memory_pool().await;
        assert_eq!(user_count(&pool).await.unwrap(), 0);
        create_user(&pool, "alice", "s3cret").await.unwrap();
        assert_eq!(user_count(&pool).await.unwrap(), 1);
        assert!(verify_login(&pool, "alice", "s3cret").await.unwrap());
        assert!(!verify_login(&pool, "alice", "wrong").await.unwrap());
        assert!(!verify_login(&pool, "ghost", "s3cret").await.unwrap());
    }

    #[tokio::test]
    async fn sessions_validate_and_revoke() {
        let pool = memory_pool().await;
        create_user(&pool, "bob", "pw").await.unwrap();
        let token = create_session(&pool, "bob").await.unwrap();
        assert_eq!(
            validate_session(&pool, &token).await.unwrap().as_deref(),
            Some("bob")
        );
        assert_eq!(validate_session(&pool, "bogus").await.unwrap(), None);
        delete_session(&pool, &token).await.unwrap();
        assert_eq!(validate_session(&pool, &token).await.unwrap(), None);
    }

    #[tokio::test]
    async fn duplicate_user_errors() {
        let pool = memory_pool().await;
        create_user(&pool, "a", "p").await.unwrap();
        assert!(create_user(&pool, "a", "p2").await.is_err());
    }

    #[tokio::test]
    async fn register_first_only_when_empty() {
        let pool = memory_pool().await;
        assert!(register_first(&pool, "admin", "pw").await.unwrap());
        // Second registration is refused atomically (table no longer empty).
        assert!(!register_first(&pool, "intruder", "pw").await.unwrap());
        assert_eq!(user_count(&pool).await.unwrap(), 1);
        assert!(verify_login(&pool, "admin", "pw").await.unwrap());
    }
}

-- Multi-user accounts + sessions. Passwords are Argon2id (salt+hash).
CREATE TABLE IF NOT EXISTS users (
    username   TEXT PRIMARY KEY,
    pw_salt    BLOB NOT NULL,
    pw_hash    BLOB NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    token      TEXT PRIMARY KEY,
    username   TEXT NOT NULL,
    expires_at INTEGER NOT NULL
);

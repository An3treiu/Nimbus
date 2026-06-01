-- Public share links. A token maps to a file path, with optional password
-- (Argon2id salt+hash) and optional expiry. Served via the public /s/<token>.
CREATE TABLE IF NOT EXISTS shares (
    token      TEXT PRIMARY KEY,
    drive      TEXT NOT NULL,
    path       TEXT NOT NULL,
    pw_salt    BLOB,
    pw_hash    BLOB,
    expires_at INTEGER,            -- unix seconds; NULL = never
    created_at INTEGER NOT NULL
);

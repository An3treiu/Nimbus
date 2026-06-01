-- Single-row vault: the data key (DEK) wrapped by the passphrase-derived key
-- and, independently, by the recovery key. The DEK itself is never stored.
CREATE TABLE IF NOT EXISTS vault (
    id                   INTEGER PRIMARY KEY CHECK (id = 1),
    salt                 BLOB NOT NULL,
    wrapped_dek          BLOB NOT NULL,
    wrapped_dek_recovery BLOB NOT NULL
);

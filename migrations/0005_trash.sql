-- Trash metadata: deleted files are moved under a .nimbus-trash/ prefix in the
-- repo (still real commits) and tracked here so they can be listed, restored,
-- or purged after a retention period.
CREATE TABLE IF NOT EXISTS trash (
    drive         TEXT NOT NULL,
    trash_path    TEXT NOT NULL,
    original_path TEXT NOT NULL,
    deleted_at    INTEGER NOT NULL,  -- unix seconds
    PRIMARY KEY (drive, trash_path)
);

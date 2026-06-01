CREATE TABLE IF NOT EXISTS cached_files (
    drive    TEXT NOT NULL,            -- "owner/repo"
    path     TEXT NOT NULL,
    kind     TEXT NOT NULL,            -- "file" | "folder"
    size     INTEGER NOT NULL DEFAULT 0,
    sha      TEXT,
    PRIMARY KEY (drive, path)
);

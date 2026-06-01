-- Per-folder UI metadata (currently a color, SharePoint-style). Keyed by the
-- folder's path within a drive. Empty color = no override (row removed).
CREATE TABLE IF NOT EXISTS folder_meta (
    drive TEXT NOT NULL,
    path  TEXT NOT NULL,
    color TEXT NOT NULL,
    PRIMARY KEY (drive, path)
);

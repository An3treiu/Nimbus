-- Single-row store for the GitHub access token obtained via OAuth device flow,
-- so it survives restarts. (A PAT supplied via env takes precedence at startup.)
CREATE TABLE IF NOT EXISTS github_token (
    id    INTEGER PRIMARY KEY CHECK (id = 1),
    token TEXT NOT NULL
);

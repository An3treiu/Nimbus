-- One embedding vector per indexed file, stored as a JSON array of floats.
-- Semantic search loads these and ranks by cosine similarity in-process.
CREATE TABLE IF NOT EXISTS embeddings (
    drive  TEXT NOT NULL,
    path   TEXT NOT NULL,
    vector TEXT NOT NULL,
    PRIMARY KEY (drive, path)
);

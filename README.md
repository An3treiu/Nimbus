# Nimbus 🌥️

Self-hosted, privacy-first cloud drive backed by your own GitHub repositories,
with pluggable AI (bring your own key or a fully local model).

**Your data, your infrastructure, your AI.**

> Status: early development (Phase 1 — foundation). See [`docs/specs/`](docs/specs/)
> for the design and [`docs/plans/`](docs/plans/) for the roadmap.

## Why Nimbus?

Nimbus sits at an intersection no mature product covers today:

- **Self-hosted + privacy-first** — runs locally or on *your* server. No telemetry, no third parties.
- **GitHub-backed storage** — files live in your own GitHub repos: durable, versioned, portable.
- **AI-native, bring-your-own** — semantic search (and later, chat over your files) powered by
  the provider *you* choose: Anthropic, OpenAI, Google, or a local model via Ollama.

## Run (Phase 1 skeleton)

```bash
export NIMBUS_GITHUB_TOKEN=ghp_xxx       # a token with `repo` scope
export NIMBUS_DRIVE_OWNER=your-username
export NIMBUS_DRIVE_REPO=your-drive-repo
export NIMBUS_DRIVE_BRANCH=main          # optional, defaults to "main"
export NIMBUS_ENCRYPTION_PASSPHRASE=...  # optional: enables client-side encryption
export NIMBUS_AI_PROVIDER=ollama         # optional: "openai" | "ollama" for search
export NIMBUS_AI_MODEL=nomic-embed-text  # optional: embedding model
cargo run --release
# -> nimbus listening on http://127.0.0.1:8080
```

## Web UI

Nimbus ships a Svelte web UI (file browser, drag-and-drop upload, download,
semantic search). Build it once, and the server serves it on the same port:

```bash
cd web && npm install && npm run build && cd ..
cargo run --release
# open http://127.0.0.1:8080
```

For frontend development with hot reload: `cd web && npm run dev` (Vite proxies
`/api` to the Rust server on :8080). Override the served directory with
`NIMBUS_WEB_DIR`.

## Encryption (zero-knowledge)

Set `NIMBUS_ENCRYPTION_PASSPHRASE` to encrypt every file client-side with
AES-256-GCM before it ever reaches GitHub. Nimbus uses **envelope encryption**:
a random data key (DEK) encrypts files and is itself wrapped by both a key
derived from your passphrase (Argon2id) and a one-time **recovery key** printed
on first run. GitHub only ever stores ciphertext; the keys never leave your
machine. Lose both the passphrase and the recovery key and the data is gone —
that is the point. Encryption binds each file's path as AES-GCM associated data,
so a ciphertext cannot be silently moved to another path. Set
`NIMBUS_RECOVERY_KEY` to unlock with the recovery key if the passphrase is lost,
and `NIMBUS_RECOVERY_KEY_OUT` to write the first-run key to a file instead of stdout.

## Semantic search (bring your own AI)

Set `NIMBUS_AI_PROVIDER` to `openai` (any OpenAI-compatible endpoint — OpenAI,
Google, LM Studio, llama.cpp) or `ollama` (fully local). Text files are embedded
on upload and ranked by cosine similarity at query time. Embeddings are computed
from plaintext **before** encryption, so search works on encrypted drives. Your
key/model are used directly — Nimbus never proxies AI through its own servers.

| Env var | Purpose |
| ------- | ------- |
| `NIMBUS_AI_PROVIDER` | `openai` or `ollama` |
| `NIMBUS_AI_BASE_URL` | endpoint (defaults per provider) |
| `NIMBUS_AI_API_KEY`  | key for OpenAI-compatible providers |
| `NIMBUS_AI_MODEL`    | embedding model |

## API

| Method | Path                  | Description                                       |
| ------ | --------------------- | ------------------------------------------------- |
| `GET`  | `/api/files`          | List files (from the local cache)                 |
| `POST` | `/api/files/<path>`   | Upload (body = raw bytes); commits to the branch  |
| `GET`  | `/api/files/<path>`   | Download                                          |
| `POST` | `/api/sync`           | Rebuild the cache from the branch's tree on GitHub |
| `GET`  | `/api/search?q=&k=`   | Semantic search over indexed files (501 if AI off) |

Uploads are durable: each one creates a blob **and** a commit on the configured
branch, so files survive GitHub's garbage collection. `GET /api/files` reads the
fast local cache; `POST /api/sync` reconciles it with the repo's actual tree.

Large files (over ~50 MB) are automatically split into chunk blobs plus a
manifest, transparently bypassing GitHub's per-blob size limit; downloads
reassemble them. Each chunk is encrypted independently when encryption is on.

## Architecture

A Cargo workspace of focused crates:

- `nimbus-core` — shared types & errors (no I/O)
- `nimbus-github` — thin GitHub Git Data API client
- `nimbus-storage` — maps the drive model onto GitHub blobs + a local SQLite cache
- `nimbus-server` — Axum HTTP server, config, cache, routes

GitHub is the source of truth; SQLite is a rebuildable local cache.

## Roadmap

| Version | Theme                    |
| ------- | ------------------------ |
| v1      | Core drive + AI search   |
| v2      | AI chat + collaboration  |
| v3      | Sync + multi-user        |
| v4      | Deploy module            |

## License

MIT

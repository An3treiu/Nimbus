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
cargo run --release
# -> nimbus listening on http://127.0.0.1:8080
```

## Encryption (zero-knowledge)

Set `NIMBUS_ENCRYPTION_PASSPHRASE` to encrypt every file client-side with
AES-256-GCM before it ever reaches GitHub. Nimbus uses **envelope encryption**:
a random data key (DEK) encrypts files and is itself wrapped by both a key
derived from your passphrase (Argon2id) and a one-time **recovery key** printed
on first run. GitHub only ever stores ciphertext; the keys never leave your
machine. Lose both the passphrase and the recovery key and the data is gone —
that is the point.

## API

| Method | Path                  | Description                                       |
| ------ | --------------------- | ------------------------------------------------- |
| `GET`  | `/api/files`          | List files (from the local cache)                 |
| `POST` | `/api/files/<path>`   | Upload (body = raw bytes); commits to the branch  |
| `GET`  | `/api/files/<path>`   | Download                                          |
| `POST` | `/api/sync`           | Rebuild the cache from the branch's tree on GitHub |

Uploads are durable: each one creates a blob **and** a commit on the configured
branch, so files survive GitHub's garbage collection. `GET /api/files` reads the
fast local cache; `POST /api/sync` reconciles it with the repo's actual tree.

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

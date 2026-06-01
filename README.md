<div align="center">

# 🌥️ Nimbus

**A self-hosted, privacy-first cloud drive that stores your files in your own GitHub repositories — with bring-your-own AI.**

*Your data. Your infrastructure. Your AI.*

[![CI](https://github.com/An3treiu/Nimbus/actions/workflows/ci.yml/badge.svg)](https://github.com/An3treiu/Nimbus/actions/workflows/ci.yml)
&nbsp;·&nbsp; Rust + Svelte &nbsp;·&nbsp; MIT licensed

</div>

---

Nimbus is a personal cloud drive (think Google Drive / Dropbox) that you run on your
own machine or server. Instead of a proprietary backend, it stores your files in a
**GitHub repository you control** — versioned, durable, and portable. Files are
**encrypted on your machine** before they ever leave it, and AI features run through
the provider *you* choose (or a fully local model). No telemetry. No middlemen.

## Why Nimbus?

No mature product today sits where Nimbus does — at the intersection of three things:

| | |
|---|---|
| 🔒 **Privacy-first & self-hosted** | Runs locally or on your server. Zero telemetry, zero third parties. |
| 🐙 **GitHub-backed storage** | Files live in *your* repos: free, durable, version-controlled, portable. |
| 🤖 **AI-native, bring-your-own** | Semantic search & "chat with your files" via OpenAI, Google, Ollama, or Anthropic — your key, your model. |

## Features

- 📁 **Drive UI** — browse, drag-and-drop upload, download, in-browser preview (images, text, Markdown, PDF).
- 🔐 **Zero-knowledge encryption** — AES-256-GCM with envelope encryption (Argon2id). GitHub only ever sees ciphertext. One-time recovery key.
- 🧩 **Durable storage** — every upload is a real Git commit, so files survive garbage collection. `Sync` reconciles the local cache with the repo.
- 📦 **Large files** — automatically chunked past GitHub's per-blob limit, transparently reassembled on download.
- 🔎 **Semantic search** — find files by meaning, not just filename (embeddings + cosine similarity).
- 💬 **Chat with your files** — retrieval-augmented Q&A over your documents.
- 🔗 **One-click GitHub login** — OAuth device flow, no PAT juggling (or use a PAT if you prefer).
- 🦀 **Single binary** — the frontend is embedded; ship one self-contained executable. Docker image too.

## Quick start

### Option A — Docker (easiest)

```bash
docker run -d --name nimbus -p 8080:8080 -v nimbus-data:/data \
  -e NIMBUS_GITHUB_TOKEN=ghp_your_token \
  -e NIMBUS_DRIVE_OWNER=your-username \
  -e NIMBUS_DRIVE_REPO=your-drive-repo \
  ghcr.io/an3treiu/nimbus:latest
# open http://localhost:8080
```

### Option B — Prebuilt binary

Download the binary for your OS from the [Releases](https://github.com/An3treiu/Nimbus/releases)
page, then:

```bash
export NIMBUS_GITHUB_TOKEN=ghp_your_token
export NIMBUS_DRIVE_OWNER=your-username
export NIMBUS_DRIVE_REPO=your-drive-repo
./nimbus            # Windows: nimbus.exe
# open http://localhost:8080
```

The frontend is embedded in the binary — nothing else to install.

### Option C — From source (local development)

Prerequisites: [Rust](https://rustup.rs) (stable) and [Node.js](https://nodejs.org) 20+.

```bash
git clone https://github.com/An3treiu/Nimbus.git
cd Nimbus

# 1. Build the web UI (embedded into the binary at compile time)
cd web && npm install && npm run build && cd ..

# 2. Configure (a GitHub token with the `repo` scope)
export NIMBUS_GITHUB_TOKEN=ghp_your_token
export NIMBUS_DRIVE_OWNER=your-username
export NIMBUS_DRIVE_REPO=your-drive-repo

# 3. Run
cargo run --release
# -> nimbus listening on http://127.0.0.1:8080
```

> **Windows / PowerShell:** use `$env:NIMBUS_GITHUB_TOKEN = "ghp_..."` instead of `export`.

**Frontend hot-reload during development:** run `cd web && npm run dev` in a second
terminal (Vite proxies `/api` to the Rust server on `:8080`).

### Getting a GitHub token

- **PAT (simplest):** [github.com/settings/tokens](https://github.com/settings/tokens) → generate a token with the **`repo`** scope → set it as `NIMBUS_GITHUB_TOKEN`.
- **OAuth (nicer):** register an OAuth App, set `NIMBUS_GITHUB_CLIENT_ID`, and click **Connect GitHub** in the UI — no token copy-pasting.

## Configuration

All configuration is via environment variables.

| Variable | Required | Default | Description |
| --- | :---: | --- | --- |
| `NIMBUS_DRIVE_OWNER` | ✅ | — | GitHub user/org that owns the drive repo |
| `NIMBUS_DRIVE_REPO` | ✅ | — | Repository used as the drive |
| `NIMBUS_GITHUB_TOKEN` | — | — | GitHub PAT (`repo` scope). Optional if using OAuth |
| `NIMBUS_GITHUB_CLIENT_ID` | — | — | OAuth App client id → enables in-app **Connect GitHub** |
| `NIMBUS_DRIVE_BRANCH` | — | `main` | Branch to store files on |
| `NIMBUS_ENCRYPTION_PASSPHRASE` | — | — | Enables client-side encryption when set |
| `NIMBUS_RECOVERY_KEY` | — | — | Unlock the vault if the passphrase is lost |
| `NIMBUS_RECOVERY_KEY_OUT` | — | — | Write the first-run recovery key to this file |
| `NIMBUS_AI_PROVIDER` | — | — | `openai` \| `ollama` \| `anthropic` (enables AI) |
| `NIMBUS_AI_API_KEY` | — | — | API key for the chosen provider |
| `NIMBUS_AI_BASE_URL` | — | per-provider | Override the provider endpoint |
| `NIMBUS_AI_MODEL` | — | per-provider | Embedding model (for search) |
| `NIMBUS_AI_CHAT_MODEL` | — | per-provider | Chat model (for chat-with-files) |
| `NIMBUS_BIND_ADDR` | — | `127.0.0.1:8080` | Listen address |
| `NIMBUS_DATABASE_URL` | — | `sqlite:nimbus.db?mode=rwc` | Local cache DB |
| `NIMBUS_WEB_DIR` | — | *(embedded)* | Serve the UI from a directory instead of the embedded copy |

## Encryption (zero-knowledge)

Set `NIMBUS_ENCRYPTION_PASSPHRASE` to encrypt every file client-side with AES-256-GCM
before it touches GitHub. Nimbus uses **envelope encryption**: a random data key (DEK)
encrypts files and is itself wrapped by both a key derived from your passphrase
(Argon2id) and a one-time **recovery key** shown on first run. Each file's path is
bound as AES-GCM associated data, so ciphertext can't be silently moved between paths.

> ⚠️ Lose **both** the passphrase and the recovery key and your data is unrecoverable — that's the point of zero-knowledge.

## AI (bring your own)

| Provider | Search (embeddings) | Chat | Notes |
| --- | :---: | :---: | --- |
| `openai` | ✅ | ✅ | Any OpenAI-compatible endpoint (OpenAI, Google, LM Studio, llama.cpp) |
| `ollama` | ✅ | ✅ | Fully local — nothing leaves your machine |
| `anthropic` | — | ✅ | Claude for chat; pair with openai/ollama for search |

Embeddings are computed from plaintext **before** encryption, so search and chat work
even on encrypted drives. Your key/model talk to your provider directly — Nimbus never
proxies AI through its own servers.

## API

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/api/files` | List files (from the local cache) |
| `POST` | `/api/files/<path>` | Upload (body = raw bytes); commits to the branch |
| `GET` | `/api/files/<path>` | Download |
| `POST` | `/api/sync` | Rebuild the cache from the repo's tree |
| `GET` | `/api/search?q=&k=` | Semantic search (`501` if AI is off) |
| `POST` | `/api/chat` | Chat with your files (`501` if AI is off) |
| `POST` | `/api/auth/device/start` | Begin GitHub OAuth device flow |
| `POST` | `/api/auth/device/poll` | Poll for the OAuth token |

## Architecture

A Cargo workspace of small, focused crates:

```
nimbus-core      shared types & errors (no I/O)
nimbus-github    GitHub Git Data API client + OAuth device flow
nimbus-crypto    AES-256-GCM, Argon2id, envelope encryption, Vault
nimbus-storage   the drive model: blobs ⇄ commits, chunking, encryption, cache
nimbus-ai        AiProvider/ChatProvider traits + OpenAI/Ollama/Anthropic
nimbus-search    embeddings index + cosine search (SQLite)
nimbus-server    Axum HTTP API, config, embedded web UI
web/             Svelte 5 + Vite frontend
```

GitHub is the source of truth; SQLite is a rebuildable local cache. The whole thing
compiles to a single binary with the UI baked in.

## Development

```bash
cargo test --all        # run the full test suite
cargo fmt --all         # format
cargo clippy --all-targets
cd web && npm run dev   # frontend with hot reload
```

CI runs format, clippy, tests and a release build on every push (`.github/workflows/ci.yml`).
Tagging `vX.Y.Z` builds binaries for Linux/Windows/macOS and a Docker image
(`.github/workflows/release.yml`).

## Roadmap

| Version | Theme |
| --- | --- |
| ✅ v1 | Core drive · durable commits · encryption · semantic search · chat · large files · web UI · OAuth |
| v2 | Streaming chat, file sharing links, auto-tagging |
| v3 | Desktop sync client, multi-user / teams |
| v4 | Optional deploy module (PaaS-style) |

## License

[MIT](LICENSE) © Nimbus contributors

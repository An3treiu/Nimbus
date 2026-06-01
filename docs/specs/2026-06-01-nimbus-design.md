# Nimbus — Design Document

**Date:** 2026-06-01
**Status:** Approved (MVP v1)

> **Nimbus** is a self-hosted, privacy-first cloud drive that stores your files in
> your own GitHub repositories, with pluggable AI (bring-your-own key or local model).
> Your data, your infrastructure, your AI.

---

## 1. Vision & Positioning

Nimbus occupies an intersection no mature product covers today:

- **Self-hosted + privacy-first** — runs locally or on *your* server. No telemetry, no third parties.
- **GitHub-backed storage** — files live in your own GitHub repos: free durable storage, full version history, portability.
- **AI-native, bring-your-own** — semantic search and (later) chat over your files, powered by the provider *you* choose: Anthropic, OpenAI, Google, or a fully local model (Ollama).

Competitors are each strong on one axis (Coolify on deploy, Frappe/ownCloud on storage) but none on all three. That intersection is Nimbus's reason to exist.

**Non-goals (v1):** We do not compete with Coolify/Dokploy on deployment/PaaS. We do not build our own object storage. We do not host or proxy any AI — the user's keys/models talk directly to their chosen provider.

---

## 2. Architecture

```
┌─────────────────────────────────────────────────────┐
│  Browser (SvelteKit SPA)                              │
│  File explorer, upload/download, preview, AI search   │
└───────────────────────┬───────────────────────────────┘
                         │ HTTP/JSON + SSE (streaming AI)
┌───────────────────────▼───────────────────────────────┐
│  Nimbus Server (Rust / Axum)  — single binary          │
│  ┌──────────┬──────────┬───────────┬────────────────┐ │
│  │ auth     │ storage  │ ai        │ index / search │ │
│  │ (OAuth)  │ engine   │(pluggable)│ (embeddings)   │ │
│  └────┬─────┴────┬─────┴─────┬─────┴───────┬────────┘ │
│       │          │           │             │           │
│  Local cache (SQLite + sqlite-vec) ────────┘           │
└───────┼──────────┼───────────┼─────────────────────────┘
        │          │           │
   GitHub API   GitHub API   Chosen provider
   (Git data)   (blobs)      (Anthropic/OpenAI/Google/Ollama)
```

- **Single Rust binary** → trivial self-hosting (`docker run ghcr.io/an3treiu/nimbus` or `./nimbus`).
- **SQLite (with `sqlite-vec`)** is a local cache + vector index for speed. The **source of truth is always GitHub** — the cache can be rebuilt from any repo.

---

## 3. Modules

Each module has one responsibility, a well-defined interface, and is independently testable.

| Module | Responsibility | Depends on |
|---|---|---|
| `auth` | GitHub OAuth flow, session management, encrypted-at-rest token storage | — |
| `storage` | File ↔ GitHub mapping, chunking of large files, optional client-side encryption | GitHub API |
| `index` | Metadata cache + vector index for fast listing and search | SQLite |
| `ai` | `AiProvider` trait + implementations (Anthropic / OpenAI / Google / Ollama) | external provider |
| `search` | Semantic search: extract text → embed → query vector index | `ai`, `index` |
| `api` | HTTP/SSE endpoints consumed by the frontend | all of the above |
| `web` (Svelte) | UI: explorer, upload, preview, search, settings | `api` |

**Boundary test:** a consumer of `storage` should never need to know it talks to GitHub; a consumer of `ai` should never need to know which provider is configured.

---

## 4. GitHub Storage Model (critical)

- Each **connected repo = one "drive".**
- File `< 100 MB` → stored as a normal blob via the Git Data API.
- File `≥ 100 MB` → **chunked**: split into parts plus a `manifest.json` linking them, transparently bypassing GitHub's per-file limit.
- **Metadata** lives in a `.nimbus/` folder in the repo (file index, virtual folder tree). This makes a drive fully portable — any clone of the repo contains everything Nimbus needs.
- **Optional client-side encryption** (AES-256-GCM): when enabled, GitHub only ever sees ciphertext. Keys are derived locally and never uploaded. This is the strongest privacy posture.

**Decision — encryption default:** client-side encryption is **opt-in** in v1 (off by default) to keep first-run UX simple, with a clear, prominent toggle in settings. Revisit defaulting-on after v1.

---

## 5. Pluggable AI (bring-your-own)

```rust
#[async_trait]
trait AiProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>>;
    async fn chat(&self, msgs: &[Message]) -> Result<TokenStream>;
}
```

Implementations: `AnthropicProvider`, `OpenAiProvider`, `GoogleProvider`, `OllamaProvider` (local).

- The user selects a provider in **Settings** and supplies a key (or a local endpoint for Ollama).
- Keys are stored **encrypted at rest** locally and are sent **only** to the user's chosen provider — never to any Nimbus-controlled service.
- A fully local model (Ollama) means **zero data leaves the machine** for AI.

**v1 uses only `embed`** (for semantic search). `chat` is defined in the trait now so v2 ("chat with your files") needs no interface change.

---

## 6. Security & Privacy

- GitHub token and AI keys are encrypted at rest with a key derived from an instance secret (set at first run).
- No telemetry, no analytics, no third-party calls. The only outbound traffic is: GitHub (storage) and the user's chosen AI provider.
- Optional client-side file encryption (Section 4).
- Sessions are HttpOnly, SameSite cookies; CSRF protection on state-changing endpoints.

---

## 7. MVP Scope (v1)

1. GitHub OAuth + connect a repo as a drive
2. Upload / download files (with chunking for ≥100 MB)
3. Folders (navigate, create)
4. Preview (images, text, markdown, PDF)
5. **Semantic AI search** with the user's chosen provider
6. Settings: choose AI provider + key/endpoint; toggle client-side encryption

**Deferred to v2+:** share links, desktop sync, multi-user/teams, the deploy/PaaS module.

---

## 8. Testing Strategy

- **Rust:** unit tests per module; integration tests for `storage` (against a mock GitHub API) and `ai` (against a mock provider).
- **Frontend:** Vitest for units; Playwright for key flows (upload, search).
- **TDD** specifically on chunking and encryption logic — the highest-risk correctness areas.

---

## 9. Roadmap (post-MVP)

| Version | Theme | Highlights |
|---|---|---|
| v1 | Core Drive + AI search | Sections 4–7 |
| v2 | AI chat + collaboration | "Chat with your files", share links, auto-tagging |
| v3 | Sync + multi-user | Desktop sync client, teams/permissions |
| v4 | Deploy module | Optional PaaS-style deploy from a connected repo |

Each version ships its own spec → plan → implementation cycle.

# Nimbus Advanced Roadmap — Competitive Research Synthesis

**Date:** 2026-06-01
**Source:** Parallel competitive research (MEGA, Nextcloud/ownCloud, Proton Drive, Filen, Seafile) + 2026 UX trends + OSS monetization, synthesized by a multi-agent workflow.

## Vision

Nimbus is the only cloud drive that combines **true zero-knowledge encryption** (Proton/MEGA-grade), **AI-native search and chat** (which none of the competitors have), and **storage in infrastructure the user already owns** (their GitHub repos or any Git remote) — shipped as a **single self-hostable Rust binary**. MEGA/Proton ask you to trust their servers; ownCloud/Nextcloud ask you to run a PHP+DB+Redis stack. Nimbus gives data sovereignty with near-zero ops.

The wedge: *"a private, version-controlled, AI-searchable drive that lives in your Git, that one person can stand up in five minutes."* Then grow into share links, real folders, multi-user spaces, and a managed cloud.

## Prioritized feature backlog

### Must
- **Real folders + tree navigation** (M) — `FileKind::Folder` exists in core but storage/list/sync only ever write `file`. Derive folders from path prefixes; breadcrumb + nested nav; `/api/files?prefix=`.
- **Delete / rename / move (+ Trash)** (M) — API can't even delete today. Each op is a Git commit. Trash = move under `.nimbus-trash/` with `deleted-at` + retention; restore = move back.
- **Version history UI** (M) — surface existing per-file Git commit history with view/restore. Near-free differentiator: every upload is already a commit.
- **Public share links** (L) — tokenized link; for encrypted drives embed the per-item key in the URL **fragment** (`#`) so it never reaches the server (MEGA/Proton model); optional password + server-side expiry. `/s/<token>` route.
- **API authentication + session layer** (M) — gate `/api/*` (session cookie post-OAuth or `NIMBUS_ADMIN_TOKEN`); CSRF + rate limiting; refuse public bind without auth. Unblocks VPS.

### Should
- **Streaming chat + inline citations** (M) — SSE token streaming; clickable citation previews; chat history; scope-to-folder.
- **Multi-user accounts + Spaces** (XL) — users (GitHub/OIDC login), per-user drives, shared Spaces with read/write roles; generalize single `drive_owner/drive_repo` into a per-user/per-space table.
- **Quotas & storage dashboard** (S) — per-drive bytes (already cached) + repo limits; usage meter; soft/hard quota on upload.

### Could
- **Auto-tagging & smart organization** (M) — embeddings-based tags/collections, "similar files"; raise the 100KB/UTF-8 indexing ceiling (PDF/docx text extraction).
- **Desktop sync client** (XL) — Rust/Tauri delta sync, selective sync, conflict handling.
- **WebDAV / S3 / CLI access** (L) — mount as network drive; scriptable.
- **Pluggable storage backends** (L) — abstract `StorageEngine` over a trait (Gitea/GitLab/S3) to de-risk the GitHub dependency (rate limits, ToS, 100MB blob cap).

## Premium tiers (open-core)

1. **Community** — free, AGPLv3, self-hosted. Full single-user drive (folders, trash, versioning), zero-knowledge encryption, bring-your-own AI, share links, WebDAV/CLI, single binary + Docker. **No feature paywall ever.**
2. **Nimbus Cloud** — managed, paid (per-GB + seat, ~$5/mo entry). We run hosting/OAuth/backups/scaling; managed-only AI add-ons (hosted embeddings/OCR, cross-repo search); one-click, no-ops, SLA. *Primary revenue line.*
3. **Teams / Enterprise** — open-core (`/ee`) + support subscription. SSO/SAML/SCIM, audit logs, RBAC on Spaces, admin console, data-residency/retention, white-labeling, access rules + secure view, paid support/LTS images.

## VPS deployment requirements

1. Ship API auth **first**; refuse to bind to `0.0.0.0` without auth configured.
2. Reverse proxy (Caddy/Traefik/Nginx) terminating TLS via Let's Encrypt; ship a `Caddyfile` + `docker-compose.yml` (nimbus + caddy).
3. Honor `X-Forwarded-For/Proto` behind a configurable trusted-proxy list.
4. Persist SQLite cache + encrypted token on a mounted volume (`/data`); document backup/restore + rebuild via `/api/sync` (Git is source of truth).
5. One-command install (docker-compose) + bare-metal single binary.
6. `/healthz` readiness endpoint + structured logging.
7. Rate limiting + brute-force protection on auth; secure cookie flags (HttpOnly, Secure, SameSite).
8. Make GitHub optional later via the storage-backend trait for air-gapped deployments.

## 2026 UI direction

- Collapsible left **sidebar** (Drive / Shared / Trash / Search / Chat / Settings), icon-rail when collapsed, persisted to localStorage.
- **Grid/list toggle** with per-folder persisted preference.
- **⌘K / Ctrl-K command palette** (go-to-file, move, share, search).
- **Dark mode** first-class via CSS custom properties + `color-scheme`, 3-way system/light/dark.
- Full **drag-and-drop** (move into folders, window-wide drop overlay) + right-click "Move to…" fallback.
- **Multi-select** + contextual bulk-action bar; roving-tabindex `role=grid` keyboard nav (Space=preview, Enter=open, Delete=trash, F2=rename).
- Optimistic UI + skeleton loaders with rollback; illustrative empty states.
- **WCAG 2.2 AA**: focus-visible, contrast, aria-live for upload/sync/chat, escapable modals with managed focus.
- Restrained glassmorphism (overlays only) with `@supports` fallbacks; low-motion gated on `prefers-reduced-motion`.
- **Streaming chat** surface with clickable citation chips.

## Build order (executing now)

1. Real folders end-to-end.
2. Delete + rename + move API (each a Git commit).
3. Trash with retention + restore.
4. Version history (Git commits per file) + view/restore.
5. API authentication; refuse public bind without it.
6. Public share links (fragment key + password + expiry).
7. Relicense MIT → AGPLv3; fence org/team features in `/ee`.
8. 2026 UI reskin: sidebar, grid/list, dark mode, ⌘K palette.

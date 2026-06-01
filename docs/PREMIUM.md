# Nimbus business model & premium tiers

Nimbus follows an **open-core / open-source-first** model (the Cal.com / Plausible
/ GitLab playbook): the self-hosted product is free and fully featured, and money
comes from a managed cloud and from org/enterprise features — not from paywalling
the core.

## Licensing recommendation

> **Recommendation (not yet applied — a relicensing decision belongs to the project owner):**
> move the core from MIT to **AGPLv3**, and place future org/team-only features in
> an `/ee` directory under a separate commercial license.

**Why AGPLv3:** it keeps self-hosting completely free while preventing a third
party from running a closed, hosted SaaS clone of Nimbus without contributing
back — exactly the protection Cal.com, Plausible, and Sentry use. The `/ee` fence
lets enterprise features (SSO, audit, RBAC) be commercially licensed without
touching the AGPL core. Switching license needs sign-off from all contributors,
so it should be done deliberately and early.

## Tiers

### 1. Community — free, self-hosted (AGPLv3)
For individuals, hobbyists, and privacy enthusiasts on their own VPS or homelab.
**No feature paywall, ever.**
- Full single-user drive: folders, trash, version history
- Zero-knowledge client-side encryption + recovery key
- Bring-your-own AI: semantic search & chat (OpenAI / Ollama / Anthropic)
- Public share links with password + expiry
- (Planned) WebDAV / CLI access
- Single binary + Docker

### 2. Nimbus Cloud — managed, paid *(primary revenue line)*
For people who want the product without running a server.
- We run hosting, the GitHub OAuth app, backups, bandwidth, and scaling
- Tiered by storage GB + bandwidth + seats; low entry price, pay-as-you-grow
- Managed-only AI add-ons that are impractical to self-host: hosted embeddings,
  OCR over files, cross-repo search
- One-click setup, no-ops, uptime SLA

### 3. Teams / Enterprise — open-core (`/ee`) + support subscription
For companies self-hosting at scale (the buyer-funded tier).
- SSO / SAML / SCIM, audit logs, granular RBAC on shared Spaces
- Admin console, data-residency & retention policies, white-labeling
- Access rules (IP / device / time), "secure view" (no-download)
- Paid support / SLA, hardened LTS images, migration assistance

## Principles

- The self-hostable core stays genuinely useful and unrestricted — trust is the
  moat for a privacy product.
- Charge for **convenience** (managed cloud) and **organizational scale**
  (enterprise governance), never for basic single-user functionality.
- Keep AI bring-your-own in the free tier; monetize only managed AI infrastructure.

---
name: add-or-update-server-feature
description: Workflow command scaffold for add-or-update-server-feature in Nimbus.
allowed_tools: ["Bash", "Read", "Write", "Grep", "Glob"]
---

# /add-or-update-server-feature

Use this workflow when working on **add-or-update-server-feature** in `Nimbus`.

## Goal

Implements or wires up new server-side features, often involving changes to multiple files in the server crate, such as src/lib.rs, src/main.rs, and sometimes Cargo.toml or new modules.

## Common Files

- `crates/nimbus-server/src/lib.rs`
- `crates/nimbus-server/src/main.rs`
- `crates/nimbus-server/src/*.rs`
- `crates/nimbus-server/Cargo.toml`

## Suggested Sequence

1. Understand the current state and failure mode before editing.
2. Make the smallest coherent change that satisfies the workflow goal.
3. Run the most relevant verification for touched files.
4. Summarize what changed and what still needs review.

## Typical Commit Signals

- Edit or add functionality in src/lib.rs
- Update or add new modules (e.g., src/routes.rs, src/config.rs, src/cache.rs)
- Update src/main.rs to wire up new features
- Optionally update Cargo.toml for dependencies

## Notes

- Treat this as a scaffold, not a hard-coded script.
- Update the command if the workflow evolves materially.
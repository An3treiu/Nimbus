---
name: add-or-update-crate-feature
description: Workflow command scaffold for add-or-update-crate-feature in Nimbus.
allowed_tools: ["Bash", "Read", "Write", "Grep", "Glob"]
---

# /add-or-update-crate-feature

Use this workflow when working on **add-or-update-crate-feature** in `Nimbus`.

## Goal

Implements a new feature or updates functionality within a specific crate, involving changes to the crate's Cargo.toml and src/lib.rs files.

## Common Files

- `crates/*/Cargo.toml`
- `crates/*/src/lib.rs`

## Suggested Sequence

1. Understand the current state and failure mode before editing.
2. Make the smallest coherent change that satisfies the workflow goal.
3. Run the most relevant verification for touched files.
4. Summarize what changed and what still needs review.

## Typical Commit Signals

- Edit or add functionality in src/lib.rs
- Update dependencies or metadata in Cargo.toml

## Notes

- Treat this as a scaffold, not a hard-coded script.
- Update the command if the workflow evolves materially.
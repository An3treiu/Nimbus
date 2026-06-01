```markdown
# Nimbus Development Patterns

> Auto-generated skill from repository analysis

## Overview

This skill teaches you the core development patterns and conventions used in the Nimbus repository, a Rust codebase structured around modular crates. You'll learn file naming, import/export styles, commit practices, and how to add or update features in both general crates and the server crate. Step-by-step workflows and command suggestions are provided to streamline contributions.

## Coding Conventions

- **File Naming:**  
  Use `camelCase` for file names.  
  _Example:_  
  ```
  src/myFeature.rs
  ```

- **Import Style:**  
  Use **relative imports** within modules.  
  _Example:_  
  ```rust
  mod utils;
  use super::utils::parse_config;
  ```

- **Export Style:**  
  Use **named exports** for module items.  
  _Example:_  
  ```rust
  pub fn start_server() { /* ... */ }
  ```

- **Commit Messages:**  
  Follow [Conventional Commits](https://www.conventionalcommits.org/) with the `feat` prefix for new features.  
  _Example:_  
  ```
  feat: add caching to config loader
  ```

## Workflows

### Add or Update Crate Feature
**Trigger:** When you want to add a new feature or update logic in any crate  
**Command:** `/new-crate-feature`

1. Edit or add functionality in `src/lib.rs` of the target crate.
2. Update dependencies or metadata in `Cargo.toml` as needed.

_Files involved:_
- `crates/*/Cargo.toml`
- `crates/*/src/lib.rs`

_Example:_
```rust
// crates/my-crate/src/lib.rs
pub fn new_feature() {
    // implementation
}
```
```toml
# crates/my-crate/Cargo.toml
[dependencies]
serde = "1.0"
```

---

### Add or Update Server Feature
**Trigger:** When you want to add new server capabilities or wire up integrations  
**Command:** `/new-server-feature`

1. Edit or add functionality in `crates/nimbus-server/src/lib.rs`.
2. Update or add new modules (e.g., `src/routes.rs`, `src/config.rs`, `src/cache.rs`).
3. Update `src/main.rs` to wire up new features.
4. Optionally update `Cargo.toml` for new dependencies.

_Files involved:_
- `crates/nimbus-server/src/lib.rs`
- `crates/nimbus-server/src/main.rs`
- `crates/nimbus-server/src/*.rs`
- `crates/nimbus-server/Cargo.toml`

_Example:_
```rust
// crates/nimbus-server/src/routes.rs
pub fn register_routes() {
    // route setup logic
}
```
```rust
// crates/nimbus-server/src/main.rs
mod routes;
fn main() {
    routes::register_routes();
    // ...
}
```
```toml
# crates/nimbus-server/Cargo.toml
[dependencies]
tokio = "1"
```

## Testing Patterns

- **Test File Pattern:**  
  Test files follow the `*.test.*` naming convention.  
  _Example:_  
  ```
  src/cache.test.rs
  ```

- **Testing Framework:**  
  The specific framework is unknown, but standard Rust test modules are likely used.  
  _Example:_  
  ```rust
  #[cfg(test)]
  mod tests {
      #[test]
      fn test_feature() {
          assert_eq!(2 + 2, 4);
      }
  }
  ```

## Commands

| Command             | Purpose                                             |
|---------------------|-----------------------------------------------------|
| /new-crate-feature  | Add or update a feature in a general crate          |
| /new-server-feature | Add or update a feature in the server crate         |
```

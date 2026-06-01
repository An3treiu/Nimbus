//! Drive activity feed: a human-readable audit trail derived from the Git
//! commit history. Every storage operation in Nimbus is a real Git commit with
//! a structured `nimbus: <action> <path>` message, so the commit log *is* the
//! audit log — no separate table to keep in sync, and it survives even if the
//! cache is wiped. This module turns those raw commit messages into typed
//! [`ActivityEvent`]s the frontend can render.

use nimbus_github::CommitInfo;
use serde::{Deserialize, Serialize};

/// The kind of change a commit represents, parsed from its message. Anything
/// that doesn't match a known Nimbus message becomes [`Action::Other`] (e.g. a
/// commit made directly on GitHub outside of Nimbus).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Upload,
    Delete,
    /// A file was sent to Trash (a move into the `.nimbus-trash/` area).
    Trash,
    /// A file was restored out of Trash back to a normal path.
    Untrash,
    Move,
    Restore,
    Other,
}

/// One entry in the activity feed: a parsed commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEvent {
    pub sha: String,
    pub date: String,
    pub action: Action,
    /// The primary path the action affected (the destination for moves).
    pub path: String,
    /// For moves, the source path; otherwise `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The raw commit message, kept for transparency / unrecognized commits.
    pub message: String,
}

const TRASH_PREFIX: &str = ".nimbus-trash/";

/// Strip the leading `.nimbus-trash/<timestamp>/` segment from a trash path,
/// recovering the original user-facing path. Falls back to the input if it
/// doesn't have the expected shape.
fn strip_trash_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(TRASH_PREFIX) {
        // rest = "<timestamp>/<original path>"
        if let Some((_, original)) = rest.split_once('/') {
            return original.to_string();
        }
    }
    path.to_string()
}

/// Parse a single Nimbus commit message into an [`Action`] plus its path(s).
/// Returns `(action, path, from)`.
fn parse_message(message: &str) -> (Action, String, Option<String>) {
    // Only the first line carries the structured summary.
    let line = message.lines().next().unwrap_or("").trim();
    let body = match line.strip_prefix("nimbus: ") {
        Some(b) => b,
        None => return (Action::Other, String::new(), None),
    };

    if let Some(rest) = body.strip_prefix("upload ") {
        return (Action::Upload, rest.to_string(), None);
    }
    if let Some(rest) = body.strip_prefix("delete ") {
        return (Action::Delete, rest.to_string(), None);
    }
    if let Some(rest) = body.strip_prefix("restore ") {
        // "restore <path> to <commit>" — a version rollback.
        if let Some((path, _commit)) = rest.rsplit_once(" to ") {
            return (Action::Restore, path.to_string(), None);
        }
        return (Action::Restore, rest.to_string(), None);
    }
    if let Some(rest) = body.strip_prefix("move ") {
        if let Some((from, to)) = rest.split_once(" -> ") {
            let from_trash = from.starts_with(TRASH_PREFIX);
            let to_trash = to.starts_with(TRASH_PREFIX);
            // Trashing = move into the trash area; untrashing = move out of it.
            if to_trash {
                return (
                    Action::Trash,
                    strip_trash_prefix(to),
                    Some(from.to_string()),
                );
            }
            if from_trash {
                return (
                    Action::Untrash,
                    to.to_string(),
                    Some(strip_trash_prefix(from)),
                );
            }
            return (Action::Move, to.to_string(), Some(from.to_string()));
        }
        return (Action::Move, rest.to_string(), None);
    }
    (Action::Other, String::new(), None)
}

/// Turn a list of commits (newest first) into activity events.
pub fn events_from_commits(commits: Vec<CommitInfo>) -> Vec<ActivityEvent> {
    commits
        .into_iter()
        .map(|c| {
            let (action, path, from) = parse_message(&c.message);
            ActivityEvent {
                sha: c.sha,
                date: c.date,
                action,
                path,
                from,
                message: c.message,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(sha: &str, message: &str) -> CommitInfo {
        CommitInfo {
            sha: sha.into(),
            message: message.into(),
            date: "2026-06-02T10:00:00Z".into(),
        }
    }

    #[test]
    fn parses_upload() {
        let (a, p, f) = parse_message("nimbus: upload docs/report.md");
        assert_eq!(a, Action::Upload);
        assert_eq!(p, "docs/report.md");
        assert_eq!(f, None);
    }

    #[test]
    fn parses_delete() {
        let (a, p, _) = parse_message("nimbus: delete old.txt");
        assert_eq!(a, Action::Delete);
        assert_eq!(p, "old.txt");
    }

    #[test]
    fn parses_plain_move() {
        let (a, p, f) = parse_message("nimbus: move a/b.txt -> c/d.txt");
        assert_eq!(a, Action::Move);
        assert_eq!(p, "c/d.txt");
        assert_eq!(f.as_deref(), Some("a/b.txt"));
    }

    #[test]
    fn move_into_trash_is_trash_with_clean_path() {
        let (a, p, f) =
            parse_message("nimbus: move notes/todo.md -> .nimbus-trash/1717326000/notes/todo.md");
        assert_eq!(a, Action::Trash);
        // The displayed path is the original, not the internal trash location.
        assert_eq!(p, "notes/todo.md");
        assert_eq!(f.as_deref(), Some("notes/todo.md"));
    }

    #[test]
    fn move_out_of_trash_is_untrash() {
        let (a, p, f) =
            parse_message("nimbus: move .nimbus-trash/1717326000/notes/todo.md -> notes/todo.md");
        assert_eq!(a, Action::Untrash);
        assert_eq!(p, "notes/todo.md");
        assert_eq!(f.as_deref(), Some("notes/todo.md"));
    }

    #[test]
    fn parses_version_restore() {
        let (a, p, _) = parse_message("nimbus: restore report.md to abc123def");
        assert_eq!(a, Action::Restore);
        assert_eq!(p, "report.md");
    }

    #[test]
    fn unknown_message_is_other() {
        let (a, p, f) = parse_message("Merge pull request #1 from feature/x");
        assert_eq!(a, Action::Other);
        assert_eq!(p, "");
        assert_eq!(f, None);
    }

    #[test]
    fn events_preserve_order_and_raw_message() {
        let commits = vec![
            commit("c2", "nimbus: upload a.txt"),
            commit("c1", "initial commit"),
        ];
        let events = events_from_commits(commits);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sha, "c2");
        assert_eq!(events[0].action, Action::Upload);
        assert_eq!(events[0].path, "a.txt");
        assert_eq!(events[1].action, Action::Other);
        assert_eq!(events[1].message, "initial commit");
    }

    #[test]
    fn other_action_serializes_lowercase() {
        let json = serde_json::to_string(&Action::Trash).unwrap();
        assert_eq!(json, "\"trash\"");
    }
}

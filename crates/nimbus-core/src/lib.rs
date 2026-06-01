use serde::{Deserialize, Serialize};

/// Whether a drive entry is a file (blob) or a folder (tree).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    File,
    Folder,
}

/// A single entry in a drive, as Nimbus models it (storage-agnostic).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveFile {
    /// POSIX-style path within the drive, e.g. "docs/notes.md".
    pub path: String,
    pub kind: FileKind,
    /// Size in bytes (0 for folders).
    pub size: u64,
    /// GitHub blob/tree SHA, if known.
    pub sha: Option<String>,
}

impl DriveFile {
    /// File name (last path segment).
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NimbusError {
    #[error("github api error: {0}")]
    GitHub(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("ai error: {0}")]
    Ai(String),
}

pub type Result<T> = std::result::Result<T, NimbusError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_returns_last_segment() {
        let f = DriveFile {
            path: "docs/sub/notes.md".into(),
            kind: FileKind::File,
            size: 12,
            sha: None,
        };
        assert_eq!(f.name(), "notes.md");
    }

    #[test]
    fn name_handles_root_level_file() {
        let f = DriveFile { path: "readme.txt".into(), kind: FileKind::File, size: 1, sha: None };
        assert_eq!(f.name(), "readme.txt");
    }
}

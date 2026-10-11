//! Workspace files and git changes as the app server reports them, plus the
//! file tree model. Free of GPUI so it can be tested headless.

mod tree;

#[cfg(test)]
mod tests;

use serde::Deserialize;

pub use tree::{FileTree, TreeRow};

/// `GET /workspace/list`: one directory level.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listing {
    pub path: String,
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Directory,
    File,
    Symlink,
    Special,
}

/// `GET /workspace/file`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileContent {
    pub path: String,
    pub size: u64,
    pub kind: FileKind,
    pub text: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Text,
    Binary,
    TooLarge,
}

/// `GET /workspace/changes`: git status relative to the session directory.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Changes {
    pub repository: bool,
    pub entries: Vec<Change>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub path: String,
    pub status: ChangeStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Conflict,
    Untracked,
    Renamed,
    Deleted,
    Added,
    Modified,
}

impl ChangeStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Conflict => "Конфликт",
            Self::Untracked => "Не отслеживается",
            Self::Renamed => "Переименован",
            Self::Deleted => "Удалён",
            Self::Added => "Добавлен",
            Self::Modified => "Изменён",
        }
    }

    pub fn mark(self) -> &'static str {
        match self {
            Self::Conflict => "!",
            Self::Untracked => "U",
            Self::Renamed => "R",
            Self::Deleted => "D",
            Self::Added => "A",
            Self::Modified => "M",
        }
    }
}

/// `GET /workspace/diff`: patch against `HEAD`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileDiff {
    pub path: String,
    pub kind: DiffKind,
    pub patch: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffKind {
    Unavailable,
    TooLarge,
    Binary,
    Text,
}

/// Highlighting language for a workspace path, by file name or extension.
pub fn language_for(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name {
        "Cargo.lock" | "Pipfile" => return Some("toml"),
        "Makefile" | "Dockerfile" | ".bashrc" | ".zshrc" | ".profile" => return Some("bash"),
        _ => {}
    }
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match extension.as_str() {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "json" | "jsonc" | "jsonl" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "sh" | "bash" | "zsh" => "bash",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "go" => "go",
        "css" | "scss" => "css",
        "html" | "htm" => "html",
        "sql" => "sql",
        "diff" | "patch" => "diff",
        _ => return None,
    })
}

/// Splits a relative path into its directory (with trailing slash) and name.
pub fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(index) => (&path[..=index], &path[index + 1..]),
        None => ("", path),
    }
}

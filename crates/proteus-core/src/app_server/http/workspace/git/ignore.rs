//! Adding workspace paths to the repository's ignore files.
use std::{fs, io::Write, path::Path};

use anyhow::{Result, ensure};
use serde::Deserialize;

use super::{
    command::run,
    pathspec, prefix, require_repository,
    status::{Changes, changes},
    toplevel,
};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IgnoreTarget {
    /// `.gitignore` at the root of the working tree.
    Gitignore,
    /// `.git/info/exclude`: local to this clone.
    Exclude,
}

/// Appends an anchored `/<path>` pattern per path, skipping lines already present.
pub(crate) fn ignore(root: &Path, paths: &[String], target: IgnoreTarget) -> Result<Changes> {
    ensure!(!paths.is_empty(), "paths must not be empty");
    require_repository(root)?;
    let prefix = prefix(root)?;
    let mut patterns = Vec::new();
    for path in paths {
        let pattern = format!("/{}", escape(&format!("{prefix}{}", pathspec(root, path)?)));
        if !patterns.contains(&pattern) {
            patterns.push(pattern);
        }
    }
    let file = match target {
        IgnoreTarget::Gitignore => toplevel(root)?.join(".gitignore"),
        IgnoreTarget::Exclude => {
            let output =
                run(root, &["rev-parse", "--git-path", "info/exclude"])?.checked("rev-parse")?;
            root.join(String::from_utf8(output.bytes)?.trim_end_matches('\n'))
        }
    };
    let existing = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let mut addition = String::new();
    if !existing.is_empty() && !existing.ends_with('\n') {
        addition.push('\n');
    }
    for pattern in patterns {
        if !existing.lines().any(|line| line.trim_end() == pattern) {
            addition.push_str(&pattern);
            addition.push('\n');
        }
    }
    if addition.trim().is_empty() {
        return changes(root);
    }
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)?
        .write_all(addition.as_bytes())?;
    changes(root)
}

/// Makes a literal path a gitignore pattern: wildcards, backslashes and a
/// trailing space are escaped.
fn escape(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(c, '\\' | '*' | '?' | '[') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    if escaped.ends_with(' ') {
        escaped.pop();
        escaped.push_str("\\ ");
    }
    escaped
}

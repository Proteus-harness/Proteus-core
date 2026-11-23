use std::path::{Path, PathBuf};

/// Top-level files for diagnostics; loading handles includes and errors separately.
pub(crate) fn config_files(config_path: Option<&Path>) -> Vec<PathBuf> {
    let Some(path) = config_path else {
        return Vec::new();
    };
    let path = super::physical_config_path(path);
    if path.is_file() {
        return vec![path];
    }
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| matches!(extension, "toml" | "json"))
                && path.is_file()
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

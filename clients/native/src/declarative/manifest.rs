//! `extension.json` of an installed extension and the folder scan.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

use crate::preferences::BadgePlace;

pub const MANIFEST: &str = "extension.json";

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Equals the folder name; tab ids and preferences use it.
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Icon name from the client's set; unknown names get the default.
    #[serde(default)]
    pub icon: Option<String>,
    /// Program and arguments, started in the extension folder.
    pub command: Vec<String>,
    pub surfaces: Surfaces,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Surfaces {
    #[serde(default)]
    pub panel: Option<PanelSurface>,
    #[serde(default)]
    pub document: Option<DocumentSurface>,
    #[serde(default)]
    pub badge: Option<BadgeSurface>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PanelSurface {
    pub title: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DocumentSurface {}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BadgeSurface {
    #[serde(default = "composer_start")]
    pub place: BadgePlace,
}

fn composer_start() -> BadgePlace {
    BadgePlace::ComposerStart
}

/// An extension found on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

/// A folder that looks like an extension but cannot be loaded.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub dir: PathBuf,
    pub error: String,
}

/// Where the user keeps extensions.
pub fn root() -> Option<PathBuf> {
    Some(crate::preferences::config_dir()?.join("extensions"))
}

/// Reads every `<root>/<id>/extension.json`, sorted by folder name. A
/// missing root means no extensions.
pub fn discover(root: &Path) -> (Vec<Installed>, Vec<Problem>) {
    let mut installed = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return (installed, problems);
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir() && !is_hidden(path))
        .collect();
    dirs.sort();
    for dir in dirs {
        match load(&dir) {
            Ok(manifest) => installed.push(Installed { dir, manifest }),
            Err(error) => problems.push(Problem {
                dir,
                error: format!("{error:#}"),
            }),
        }
    }
    (installed, problems)
}

/// Dot folders are unfinished installs and anything else the user hid.
fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn load(dir: &Path) -> Result<Manifest> {
    let path = dir.join(MANIFEST);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let manifest = parse(&text).with_context(|| format!("invalid {}", path.display()))?;
    let folder = dir.file_name().and_then(|name| name.to_str()).unwrap_or("");
    if manifest.id != folder {
        bail!("id `{}` must equal the folder name `{folder}`", manifest.id);
    }
    Ok(manifest)
}

/// Reads and checks an `extension.json` text.
pub fn parse(text: &str) -> Result<Manifest> {
    let manifest: Manifest = serde_json::from_str(text)?;
    if !valid_id(&manifest.id) {
        bail!("id must be lowercase latin letters, digits and dashes");
    }
    if manifest.title.trim().is_empty() {
        bail!("title is empty");
    }
    if manifest
        .command
        .first()
        .is_none_or(|program| program.is_empty())
    {
        bail!("command is empty");
    }
    let surfaces = &manifest.surfaces;
    if surfaces.panel.is_none() && surfaces.document.is_none() && surfaces.badge.is_none() {
        bail!("no surfaces");
    }
    if surfaces.badge.as_ref().is_some_and(|badge| badge.place.is_rail()) {
        bail!("a badge with text does not fit the rail");
    }
    Ok(manifest)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !id.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, folder: &str, text: &str) {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST), text).unwrap();
    }

    #[test]
    fn discovers_valid_extensions_and_reports_broken_ones() {
        let root = std::env::temp_dir().join(format!("proteus-ext-{}", uuid::Uuid::new_v4()));
        write(
            &root,
            "todo",
            r#"{"id": "todo", "title": "Задачи", "command": ["python3", "main.py"],
                "surfaces": {"panel": {"title": "Задачи"}, "badge": {}}}"#,
        );
        write(
            &root,
            "other",
            r#"{"id": "todo", "title": "x", "command": ["x"], "surfaces": {"document": {}}}"#,
        );
        write(
            &root,
            "bad",
            r#"{"id": "bad", "title": "x", "command": ["x"], "surfaces": {}}"#,
        );
        std::fs::create_dir_all(root.join("empty")).unwrap();

        let (installed, problems) = discover(&root);
        std::fs::remove_dir_all(&root).unwrap();

        assert_eq!(installed.len(), 1);
        let todo = &installed[0].manifest;
        assert_eq!(todo.id, "todo");
        assert_eq!(
            todo.surfaces.badge.as_ref().unwrap().place,
            BadgePlace::ComposerStart
        );
        let broken: Vec<_> = problems
            .iter()
            .map(|problem| problem.dir.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(broken, ["bad", "empty", "other"]);
    }

    #[test]
    fn ids_are_plain_slugs() {
        assert!(valid_id("git-stats2"));
        assert!(!valid_id("Git"));
        assert!(!valid_id("a:b"));
        assert!(!valid_id("-a"));
    }
}

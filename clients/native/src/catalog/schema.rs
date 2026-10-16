//! Strict index, with per-entry diagnostics so one broken item does not
//! hide the rest of a catalog. Paths are always relative to the checkout.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub const INDEX: &str = "proteus-catalog.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    ClientExtension,
    AgentPlugin,
    Skill,
    McpServer,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Self::ClientExtension => "Расширение клиента",
            Self::AgentPlugin => "Agent Plugin",
            Self::Skill => "Skill",
            Self::McpServer => "MCP-сервер",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub category: String,
    pub version: String,
    pub path: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    pub format: u32,
    pub title: String,
    pub categories: Vec<Category>,
    pub entries: Vec<serde_json::Value>,
}

#[derive(Clone)]
pub struct Catalog {
    pub root: PathBuf,
    pub title: String,
    pub categories: Vec<Category>,
    pub entries: Vec<Entry>,
    pub problems: Vec<String>,
}

pub fn slug(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// Reject absolute paths, parent traversal and repository metadata.
pub fn relative(path: &Path) -> Result<()> {
    ensure!(!path.as_os_str().is_empty(), "Пустой путь");
    ensure!(
        path.components().all(|part| match part {
            Component::Normal(name) => name != ".git",
            Component::CurDir => true,
            _ => false,
        }),
        "Путь должен оставаться внутри каталога: {}",
        path.display()
    );
    Ok(())
}

pub fn directory(root: &Path, entry: &Entry) -> Result<PathBuf> {
    relative(&entry.path)?;
    let root = root.canonicalize()?;
    let path = root.join(&entry.path).canonicalize()?;
    ensure!(
        path.starts_with(&root) && path.is_dir(),
        "Папка выходит за пределы каталога"
    );
    Ok(path)
}

/// A repository without the index is read as an Agent Skills repository;
/// `name` titles it then and `trees` versions its folders: a git tree id by
/// the folder path relative to `root`, the root itself being empty.
pub fn read(root: &Path, name: &str, trees: &BTreeMap<String, String>) -> Result<Catalog> {
    if !root.join(INDEX).exists() {
        return super::skills::read(root, name.to_owned(), trees);
    }
    let text = std::fs::read_to_string(root.join(INDEX))
        .context("Не удалось прочитать proteus-catalog.json")?;
    let index: Index = serde_json::from_str(&text).context("Неверный индекс каталога")?;
    ensure!(
        index.format == 1,
        "Неподдержанная версия каталога: {}",
        index.format
    );
    ensure!(!index.title.trim().is_empty(), "У каталога нет названия");
    let mut categories = HashSet::new();
    for category in &index.categories {
        ensure!(
            slug(&category.id) && !category.title.trim().is_empty(),
            "Неверная категория"
        );
        ensure!(
            categories.insert(category.id.as_str()),
            "Повторная категория: {}",
            category.id
        );
    }
    let mut entries = Vec::new();
    let mut problems = Vec::new();
    let mut seen = HashSet::new();
    let mut duplicates = HashSet::new();
    for (ix, value) in index.entries.into_iter().enumerate() {
        let parsed = (|| -> Result<Entry> {
            let entry: Entry = serde_json::from_value(value)?;
            ensure!(slug(&entry.id), "Неверный id");
            if !seen.insert(entry.id.clone()) {
                duplicates.insert(entry.id.clone());
            }
            ensure!(
                !entry.title.trim().is_empty() && !entry.version.trim().is_empty(),
                "Нужны title и version"
            );
            ensure!(
                categories.contains(entry.category.as_str()),
                "Неизвестная категория: {}",
                entry.category
            );
            directory(root, &entry)?;
            Ok(entry)
        })();
        match parsed {
            Ok(entry) => entries.push(entry),
            Err(error) => problems.push(format!("Запись {}: {error:#}", ix + 1)),
        }
    }
    for id in &duplicates {
        problems.push(format!("Повторный id: {id}"));
    }
    entries.retain(|entry| !duplicates.contains(&entry.id));
    Ok(Catalog {
        root: root.to_owned(),
        title: index.title,
        categories: index.categories,
        entries,
        problems,
    })
}

/// README.md of the entry; a skill without one shows its instructions.
pub fn readme(catalog: &Catalog, entry: &Entry) -> Result<Option<String>> {
    let dir = directory(&catalog.root, entry)?;
    let skill = entry.kind == Kind::Skill && !dir.join("README.md").exists();
    let path = dir.join(if skill { "SKILL.md" } else { "README.md" });
    if !path.exists() {
        return Ok(None);
    }
    let resolved = path.canonicalize()?;
    ensure!(
        resolved.starts_with(catalog.root.canonicalize()?),
        "README выходит за пределы каталога"
    );
    ensure!(
        std::fs::metadata(&resolved)?.len() <= 512 * 1024,
        "README больше 512 КиБ"
    );
    let text = std::fs::read_to_string(resolved)?;
    Ok(Some(if skill {
        super::skills::body(&text).to_owned()
    } else {
        text
    }))
}

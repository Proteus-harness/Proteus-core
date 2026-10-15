//! Agent Skills repositories without a Proteus index. Every folder with a
//! SKILL.md is one entry, checked like the agent's own skill discovery. The
//! whole folder is installed, so folders nested in a skill belong to it.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use super::schema::{Catalog, Category, Entry, INDEX, Kind};
use crate::packages::Contents;

const FILE: &str = "SKILL.md";
const MAX_DEPTH: usize = 8;
const MAX_SKILLS: usize = 1000;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_DESCRIPTION: usize = 1024;
/// Category of a skill at the repository root.
const ROOT: &str = ".";

#[derive(Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
}

pub fn read(root: &Path, title: String) -> Result<Catalog> {
    let mut found = Vec::new();
    find(root, Path::new(""), 0, &mut found)?;
    ensure!(
        !found.is_empty(),
        "В репозитории нет {INDEX} и ни одного {FILE}"
    );
    let mut entries = Vec::new();
    let mut problems = Vec::new();
    for dir in found {
        match entry(root, &dir) {
            Ok(entry) => entries.push(entry),
            Err(error) => problems.push(format!("{}: {error:#}", dir.join(FILE).display())),
        }
    }
    // Two folders would install into one skills/<name> directory.
    let mut places = BTreeMap::<String, Vec<String>>::new();
    for entry in &entries {
        places
            .entry(entry.id.clone())
            .or_default()
            .push(entry.path.display().to_string());
    }
    for (id, paths) in &places {
        if paths.len() > 1 {
            problems.push(format!("Повторный skill {id}: {}", paths.join(", ")));
        }
    }
    entries.retain(|entry| places[&entry.id].len() == 1);
    let mut categories = Vec::<Category>::new();
    for entry in &entries {
        if !categories.iter().any(|old| old.id == entry.category) {
            categories.push(Category {
                id: entry.category.clone(),
                title: if entry.category == ROOT {
                    title.clone()
                } else {
                    entry.category.clone()
                },
            });
        }
    }
    Ok(Catalog {
        root: root.to_owned(),
        title,
        categories,
        entries,
        problems,
    })
}

fn find(root: &Path, dir: &Path, depth: usize, found: &mut Vec<PathBuf>) -> Result<()> {
    let path = root.join(dir);
    if path.join(FILE).is_file() {
        ensure!(
            found.len() < MAX_SKILLS,
            "В репозитории больше {MAX_SKILLS} skills"
        );
        found.push(dir.to_owned());
        return Ok(());
    }
    if depth == MAX_DEPTH {
        return Ok(());
    }
    let mut children = std::fs::read_dir(&path)?
        .filter_map(|child| child.ok())
        .filter(|child| {
            child.file_name() != ".git" && child.file_type().is_ok_and(|kind| kind.is_dir())
        })
        .map(|child| child.file_name())
        .collect::<Vec<_>>();
    children.sort();
    for name in children {
        find(root, &dir.join(name), depth + 1, found)?;
    }
    Ok(())
}

fn entry(root: &Path, dir: &Path) -> Result<Entry> {
    let folder = root.join(dir);
    ensure!(
        std::fs::metadata(folder.join(FILE))?.len() <= MAX_FILE_BYTES,
        "{FILE} больше 256 КиБ"
    );
    let files = Contents::folder(&folder)?;
    let (yaml, _) = split(files.text(FILE)?)?;
    let front: Frontmatter = serde_yaml::from_str(yaml).context("Неверный YAML frontmatter")?;
    ensure!(name(&front.name), "Неверное имя skill: {}", front.name);
    if let Some(folder) = dir.file_name() {
        ensure!(
            folder == front.name.as_str(),
            "Имя {} не совпадает с папкой {}",
            front.name,
            folder.to_string_lossy()
        );
    }
    let description = front.description.trim();
    ensure!(!description.is_empty(), "Пустое описание");
    ensure!(
        description.chars().count() <= MAX_DESCRIPTION,
        "Описание длиннее {MAX_DESCRIPTION} символов"
    );
    let category = dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| ROOT.to_string(), |parent| parent.display().to_string());
    Ok(Entry {
        id: front.name.clone(),
        kind: Kind::Skill,
        title: front.name,
        description: description.to_owned(),
        category,
        version: files.fingerprint(),
        path: if dir.as_os_str().is_empty() {
            PathBuf::from(ROOT)
        } else {
            dir.to_owned()
        },
    })
}

/// The same rule as the agent's skill discovery.
fn name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=64).contains(&bytes.len())
        && !name.contains("--")
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn split(content: &str) -> Result<(&str, &str)> {
    let start = if content.starts_with("---\n") {
        4
    } else if content.starts_with("---\r\n") {
        5
    } else {
        bail!("Нет YAML frontmatter");
    };
    let mut offset = start;
    for line in content[start..].split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return Ok((&content[start..offset], &content[offset + line.len()..]));
        }
        offset += line.len();
    }
    bail!("YAML frontmatter не закрыт")
}

/// Instructions without the frontmatter, for the entry details.
pub fn body(content: &str) -> &str {
    split(content).map_or(content, |(_, body)| body).trim()
}

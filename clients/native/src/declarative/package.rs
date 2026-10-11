//! Installing extensions from ZIP archives and removing them.
//!
//! The archive holds the extension folder contents with `extension.json` at
//! its root. It is unpacked into a hidden folder first and renamed into
//! `<root>/<id>` only when complete, so a broken archive leaves nothing
//! behind.

use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};

use super::manifest::{MANIFEST, Manifest, parse};

const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 128 * 1024 * 1024;
const MAX_FILES: usize = 4096;

struct File {
    content: Vec<u8>,
    mode: Option<u32>,
}

/// Unpacks the archive at `archive` into `<root>/<id>`. `taken` are ids the
/// new extension must not reuse besides the folders already in `root`.
pub fn install(root: &Path, archive: &Path, taken: &[&str]) -> Result<Manifest> {
    let size = std::fs::metadata(archive)
        .with_context(|| format!("Не удалось прочитать {}", archive.display()))?
        .len();
    ensure!(size <= MAX_ARCHIVE_BYTES, "ZIP превышает 64 МиБ");
    let bytes = std::fs::read(archive)
        .with_context(|| format!("Не удалось прочитать {}", archive.display()))?;
    install_bytes(root, &bytes, taken)
}

fn install_bytes(root: &Path, bytes: &[u8], taken: &[&str]) -> Result<Manifest> {
    let files = unpack(bytes)?;
    let text = files
        .get(Path::new(MANIFEST))
        .context("В корне ZIP нужен extension.json")?;
    let text = std::str::from_utf8(&text.content).context("extension.json не в UTF-8")?;
    let manifest = parse(text).context("Неверный extension.json")?;
    let destination = root.join(&manifest.id);
    ensure!(
        !taken.contains(&manifest.id.as_str()) && !destination.exists(),
        "Расширение `{}` уже установлено или его имя занято",
        manifest.id
    );
    std::fs::create_dir_all(root)
        .with_context(|| format!("Не удалось создать {}", root.display()))?;
    let temporary = root.join(format!(".install-{}", uuid::Uuid::new_v4()));
    let result = write(&temporary, &files).and_then(|()| {
        std::fs::rename(&temporary, &destination)
            .with_context(|| format!("Не удалось создать {}", destination.display()))
    });
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    result.map(|()| manifest)
}

fn unpack(bytes: &[u8]) -> Result<BTreeMap<PathBuf, File>> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).context("Не удалось прочитать ZIP")?;
    ensure!(
        archive.len() <= MAX_FILES,
        "В ZIP больше {MAX_FILES} записей"
    );
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let path = relative_path(entry.name())?;
        let mode = entry.unix_mode();
        ensure!(
            mode.is_none_or(|mode| mode & 0o170000 != 0o120000),
            "Ссылки в ZIP не поддерживаются"
        );
        if entry.is_dir() {
            continue;
        }
        let size = entry.size();
        total = total.checked_add(size).context("Неверный размер ZIP")?;
        ensure!(
            total <= MAX_EXPANDED_BYTES,
            "Распакованный ZIP превышает 128 МиБ"
        );
        let mut content = Vec::new();
        entry.by_ref().take(size + 1).read_to_end(&mut content)?;
        ensure!(content.len() as u64 == size, "Неверный размер файла в ZIP");
        let file = File { content, mode };
        ensure!(
            files.insert(path, file).is_none(),
            "Повторный путь файла в ZIP"
        );
    }
    Ok(files)
}

fn relative_path(name: &str) -> Result<PathBuf> {
    let path = Path::new(name);
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            _ => bail!("Путь `{name}` выходит за пределы расширения"),
        }
    }
    ensure!(!clean.as_os_str().is_empty(), "Пустой путь в ZIP");
    Ok(clean)
}

fn write(dir: &Path, files: &BTreeMap<PathBuf, File>) -> Result<()> {
    std::fs::create_dir(dir).with_context(|| format!("Не удалось создать {}", dir.display()))?;
    for (path, file) in files {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, &file.content)
            .with_context(|| format!("Не удалось записать {}", target.display()))?;
        if let Some(mode) = file.mode {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(mode & 0o755 | 0o600);
            std::fs::set_permissions(&target, permissions)?;
        }
    }
    Ok(())
}

/// Deletes the folder of an installed extension.
pub fn remove(root: &Path, id: &str) -> Result<()> {
    let dir = root.join(id);
    ensure!(
        dir.parent() == Some(root) && !id.is_empty() && !id.starts_with('.'),
        "Неверное имя расширения `{id}`"
    );
    std::fs::remove_dir_all(&dir).with_context(|| format!("Не удалось удалить {}", dir.display()))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use zip::write::SimpleFileOptions;

    use super::*;

    const MANIFEST_TEXT: &str = r#"{"id": "todo", "title": "Задачи", "command": ["./run"],
        "surfaces": {"panel": {"title": "Задачи"}}}"#;

    fn archive(files: &[(&str, &str, u32)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text, mode) in files {
            let options = SimpleFileOptions::default().unix_permissions(*mode);
            writer.start_file(*name, options).unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("proteus-zip-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn installs_once_keeping_executables_and_removes() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root();
        let bytes = archive(&[
            ("extension.json", MANIFEST_TEXT, 0o644),
            ("run", "#!/bin/sh\n", 0o755),
            ("lib/data.txt", "x", 0o644),
        ]);

        let manifest = install_bytes(&root, &bytes, &[]).unwrap();
        assert_eq!(manifest.id, "todo");
        let mode = std::fs::metadata(root.join("todo/run"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111);
        assert!(root.join("todo/lib/data.txt").is_file());
        assert!(install_bytes(&root, &bytes, &[]).is_err());

        remove(&root, "todo").unwrap();
        let left: Vec<_> = std::fs::read_dir(&root).unwrap().collect();
        std::fs::remove_dir_all(&root).unwrap();
        assert!(left.is_empty());
    }

    #[test]
    fn rejects_escaping_paths_taken_ids_and_missing_manifest() {
        let root = temp_root();
        let escaping = archive(&[
            ("extension.json", MANIFEST_TEXT, 0o644),
            ("../evil", "x", 0o644),
        ]);
        assert!(install_bytes(&root, &escaping, &[]).is_err());
        let fine = archive(&[("extension.json", MANIFEST_TEXT, 0o644)]);
        assert!(install_bytes(&root, &fine, &["todo"]).is_err());
        let nested = archive(&[("todo/extension.json", MANIFEST_TEXT, 0o644)]);
        assert!(install_bytes(&root, &nested, &[]).is_err());
        assert!(!root.exists() || std::fs::read_dir(&root).unwrap().next().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}

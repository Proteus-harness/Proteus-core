//! Opaque cache revisions of a HEAD-to-worktree diff. A revision combines
//! the actual full HEAD, this path's index stages and filesystem metadata;
//! it is an invalidation hint, not a cryptographic content digest.
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::Path,
};

/// The same canonical stage record for porcelain-v2 status and ls-files.
pub(super) fn stage(mode: &str, oid: &str, number: u8) -> String {
    if mode == "000000" {
        return String::new();
    }
    format!("{mode} {oid} {number}\0")
}

pub(super) fn stages(bytes: &[u8]) -> Option<String> {
    let mut result = String::new();
    for record in super::complete_records(bytes) {
        let end = record.iter().position(|b| *b == b'\t')?;
        let text = std::str::from_utf8(&record[..end]).ok()?;
        let mut fields = text.split_whitespace();
        let (mode, oid, number) = (fields.next()?, fields.next()?, fields.next()?);
        if fields.next().is_some() {
            return None;
        }
        result.push_str(&stage(mode, oid, number.parse().ok()?));
    }
    Some(result)
}

/// Missing paths are versioned too: a deletion's diff still depends on
/// HEAD and the index. Unsupported/inaccessible metadata is not cacheable.
pub(super) fn token(root: &Path, path: &str, head: Option<&str>, index: &str) -> Option<String> {
    let mut hash = DefaultHasher::new();
    "git-source-v1".hash(&mut hash);
    path.hash(&mut hash);
    head.hash(&mut hash);
    index.hash(&mut hash);
    match fs::symlink_metadata(root.join(path)) {
        Ok(meta) => {
            true.hash(&mut hash);
            meta.is_file().hash(&mut hash);
            meta.is_dir().hash(&mut hash);
            meta.file_type().is_symlink().hash(&mut hash);
            meta.len().hash(&mut hash);
            meta.modified().ok()?.hash(&mut hash);
            meta.permissions().readonly().hash(&mut hash);
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                meta.dev().hash(&mut hash);
                meta.ino().hash(&mut hash);
                meta.mode().hash(&mut hash);
                meta.ctime().hash(&mut hash);
                meta.ctime_nsec().hash(&mut hash);
            }
            #[cfg(not(unix))]
            meta.created().ok().hash(&mut hash);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false.hash(&mut hash),
        Err(_) => return None,
    }
    Some(format!("{:016x}", hash.finish()))
}

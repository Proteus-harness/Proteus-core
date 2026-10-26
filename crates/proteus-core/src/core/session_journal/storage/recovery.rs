use std::{
    fs::{self, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

pub(super) fn repair_unterminated_tail(path: &Path) -> Result<()> {
    let mut source = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let metadata = source.metadata()?;
    if !metadata.is_file() {
        bail!("journal {} is not a regular file", path.display());
    }
    let length = metadata.len();
    let complete = super::read::complete_jsonl_offset(&mut source, length)?;
    if complete == length {
        return Ok(());
    }
    drop(source);
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open {} for tail recovery", path.display()))?;
    file.set_len(complete)
        .with_context(|| format!("failed to truncate interrupted tail in {}", path.display()))?;
    file.sync_data()?;
    Ok(())
}

pub(super) struct AppendRollback {
    path: PathBuf,
    committed_offset: u64,
    armed: bool,
}

impl AppendRollback {
    pub(super) fn new(path: PathBuf, committed_offset: u64) -> Self {
        Self {
            path,
            committed_offset,
            armed: true,
        }
    }

    pub(super) fn commit(mut self) {
        self.armed = false;
    }

    pub(super) fn rollback(&mut self) -> Result<()> {
        restore_committed_offset(&self.path, self.committed_offset)?;
        self.armed = false;
        Ok(())
    }
}

impl Drop for AppendRollback {
    fn drop(&mut self) {
        if self.armed {
            let _ = restore_committed_offset(&self.path, self.committed_offset);
        }
    }
}

pub(super) fn restore_committed_offset(path: &Path, committed_offset: u64) -> Result<()> {
    let actual_len = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == ErrorKind::NotFound && committed_offset == 0 => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect journal {}", path.display()));
        }
    };
    if actual_len < committed_offset {
        bail!(
            "journal {} is shorter than committed offset: {actual_len} < {committed_offset}",
            path.display()
        );
    }
    if actual_len == committed_offset {
        return Ok(());
    }
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open {} for append recovery", path.display()))?;
    file.set_len(committed_offset)
        .with_context(|| format!("failed to restore committed offset in {}", path.display()))?;
    file.sync_data()
        .with_context(|| format!("failed to sync append recovery for {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_recovery_handles_block_boundaries_and_non_utf8_without_changing_complete_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        for prefix_len in [0, 8192, 8193] {
            for tail_len in [0, 1, 8191, 8192, 8193, 32769] {
                let mut prefix = vec![b'x'; prefix_len];
                if let Some(last) = prefix.last_mut() {
                    *last = b'\n';
                }
                let mut original = prefix.clone();
                original.extend(std::iter::repeat_n(0xff, tail_len));
                fs::write(&path, &original).unwrap();
                let mut file = fs::File::open(&path).unwrap();
                assert_eq!(
                    super::super::read::complete_jsonl_offset(&mut file, original.len() as u64)
                        .unwrap(),
                    prefix_len as u64
                );
                assert_eq!(
                    fs::read(&path).unwrap(),
                    original,
                    "inspection must be read-only"
                );
                repair_unterminated_tail(&path).unwrap();
                assert_eq!(fs::read(&path).unwrap(), prefix);
            }
        }
    }

    #[test]
    fn armed_guard_removes_even_a_complete_uncommitted_record() {
        let dir = tempfile::tempdir().expect("journal dir");
        let path = dir.path().join("journal.jsonl");
        fs::write(&path, b"committed\nuncommitted\n").expect("journal");

        let guard = AppendRollback::new(path.clone(), b"committed\n".len() as u64);
        drop(guard);

        assert_eq!(fs::read(path).expect("recovered journal"), b"committed\n");
    }

    #[test]
    fn recovery_refuses_to_extend_a_shorter_journal() {
        let dir = tempfile::tempdir().expect("journal dir");
        let path = dir.path().join("journal.jsonl");
        fs::write(&path, b"short").expect("journal");

        let error = restore_committed_offset(&path, 100).expect_err("short journal must fail");

        assert!(error.to_string().contains("shorter than committed offset"));
        assert_eq!(fs::read(path).expect("unchanged journal"), b"short");
    }
}

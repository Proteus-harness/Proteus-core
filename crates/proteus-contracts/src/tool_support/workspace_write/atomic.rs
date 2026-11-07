//! A private sibling is published only after all bytes have been written.
use super::*;

/// Descriptor-bound replacement. Dropping without `commit` keeps the old file.
#[derive(Debug)]
pub struct WorkspaceAtomicFile {
    file: File,
    path: PathBuf,
    #[cfg(unix)]
    directory: File,
    #[cfg(unix)]
    temporary: std::ffi::OsString,
    published: bool,
}

impl WorkspaceAtomicFile {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Sync the completed file, atomically replace this directory entry, then
    /// sync the containing directory. No string path is reopened for writing.
    pub fn commit(mut self) -> Result<(), String> {
        self.file.sync_all().map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            rustix::fs::renameat(
                &self.directory,
                &self.temporary,
                &self.directory,
                self.path.file_name().expect("validated leaf"),
            )
            .map_err(|error| format!("cannot publish workspace file: {error}"))?;
            self.published = true;
            self.directory
                .sync_all()
                .map_err(|error| error.to_string())?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err("descriptor-relative workspace writes require a Unix host".into())
        }
    }
}

impl Write for WorkspaceAtomicFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Drop for WorkspaceAtomicFile {
    fn drop(&mut self) {
        #[cfg(unix)]
        if !self.published {
            let _ = rustix::fs::unlinkat(
                &self.directory,
                &self.temporary,
                rustix::fs::AtFlags::empty(),
            );
        }
    }
}

/// Prepare an atomic replacement under the workspace, rejecting symlink
/// parents and an existing symlink/non-regular target. Hardlink aliases are
/// replaced independently: the shared original inode is never truncated.
pub fn workspace_file_for_atomic_write(
    cwd: &Path,
    path: &Path,
) -> Result<WorkspaceAtomicFile, String> {
    #[cfg(unix)]
    {
        use rustix::fs::{AtFlags, FileType, Mode, OFlags, openat, statat};
        let prepared = PreparedWrite::new(cwd, path)?;
        let directory = prepared.parent_directory()?;
        let leaf = prepared.relative.file_name().expect("validated leaf");
        let (mode, existing) = match statat(&directory, leaf, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(metadata) => {
                if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile {
                    return Err("refusing symlink or non-regular workspace target".into());
                }
                (Mode::from_raw_mode(metadata.st_mode & 0o777), true)
            }
            Err(rustix::io::Errno::NOENT) => (Mode::from_raw_mode(0o666), false),
            Err(error) => return Err(error.to_string()),
        };
        let temporary =
            std::ffi::OsString::from(format!(".proteus-write-{}.tmp", uuid::Uuid::new_v4()));
        let file = File::from(
            openat(
                &directory,
                &temporary,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                mode,
            )
            .map_err(|error| format!("cannot stage workspace file: {error}"))?,
        );
        let staged = WorkspaceAtomicFile {
            file,
            path: prepared.path,
            directory,
            temporary,
            published: false,
        };
        if existing {
            use std::os::unix::fs::PermissionsExt as _;
            staged
                .file
                .set_permissions(std::fs::Permissions::from_mode(mode.as_raw_mode()))
                .map_err(|error| error.to_string())?;
        }
        Ok(staged)
    }
    #[cfg(not(unix))]
    {
        let _ = (cwd, path);
        Err("descriptor-relative workspace writes require a Unix host".into())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn abandoned_or_partial_stage_preserves_original_and_cleans_up() {
        let workspace = tempfile::tempdir().unwrap();
        let target = workspace.path().join("file");
        std::fs::write(&target, "original").unwrap();
        let mut write =
            workspace_file_for_atomic_write(workspace.path(), Path::new("file")).unwrap();
        write.write_all(b"partial").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        drop(write);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        assert_eq!(std::fs::read_dir(workspace.path()).unwrap().count(), 1);
    }

    #[test]
    fn commit_replaces_hardlink_alias_without_modifying_external_inode() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let external = outside.path().join("original");
        std::fs::write(&external, "external").unwrap();
        std::fs::hard_link(&external, workspace.path().join("alias")).unwrap();
        let mut write =
            workspace_file_for_atomic_write(workspace.path(), Path::new("alias")).unwrap();
        write.write_all(b"replacement").unwrap();
        write.commit().unwrap();
        assert_eq!(std::fs::read_to_string(&external).unwrap(), "external");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("alias")).unwrap(),
            "replacement"
        );
    }

    #[test]
    fn replacement_preserves_existing_permission_bits() {
        use std::os::unix::fs::PermissionsExt as _;
        let workspace = tempfile::tempdir().unwrap();
        let target = workspace.path().join("script");
        std::fs::write(&target, "old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o764)).unwrap();
        let mut staged =
            workspace_file_for_atomic_write(workspace.path(), Path::new("script")).unwrap();
        staged.write_all(b"new").unwrap();
        staged.commit().unwrap();
        assert_eq!(
            target.metadata().unwrap().permissions().mode() & 0o777,
            0o764
        );
    }

    #[test]
    fn changed_parent_cannot_redirect_atomic_publication() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let mut write =
            workspace_file_for_atomic_write(workspace.path(), Path::new("dir/new")).unwrap();
        write.write_all(b"inside").unwrap();
        std::fs::rename(
            workspace.path().join("dir"),
            workspace.path().join("parked"),
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path(), workspace.path().join("dir")).unwrap();
        write.commit().unwrap();
        assert!(!outside.path().join("new").exists());
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("parked/new")).unwrap(),
            "inside"
        );
    }
}

//! Descriptor-bound workspace writes. Checking a path never authorizes a later
//! std::fs::write of that path; callers receive the opened file instead.
use std::{
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct WorkspaceFile {
    file: File,
    path: PathBuf,
}

impl WorkspaceFile {
    /// Display metadata only; writes must use this handle's Write impl.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Write for WorkspaceFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Opens/truncates a file below cwd without following a symlink in any path
/// component. Missing parents are created relative to directory handles.
pub fn workspace_file_for_write(cwd: &Path, path: &Path) -> Result<WorkspaceFile, String> {
    #[cfg(unix)]
    {
        PreparedWrite::new(cwd, path)?.open()
    }
    #[cfg(not(unix))]
    {
        let _ = (cwd, path);
        Err("descriptor-relative workspace writes require a Unix host".into())
    }
}

#[cfg(unix)]
struct PreparedWrite {
    root: File,
    relative: PathBuf,
    path: PathBuf,
}

#[cfg(unix)]
impl PreparedWrite {
    fn new(cwd: &Path, path: &Path) -> Result<Self, String> {
        use rustix::fs::{Mode, OFlags, open};
        let base = cwd.canonicalize().map_err(|e| e.to_string())?;
        let relative = if path.is_absolute() {
            path.strip_prefix(&base)
                .map_err(|_| format!("path escapes workspace: {}", path.display()))?
        } else {
            path
        };
        let relative = super::safe_relative_path(relative)?;
        let fd = open(
            &base,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|e| format!("cannot open workspace root: {e}"))?;
        Ok(Self {
            root: fd.into(),
            path: base.join(&relative),
            relative,
        })
    }

    fn open(self) -> Result<WorkspaceFile, String> {
        use rustix::fs::{Mode, OFlags, mkdirat, openat};
        let parents = self.relative.parent().unwrap_or(Path::new(""));
        let mut directory = self.root.try_clone().map_err(|e| e.to_string())?;
        for component in parents.components() {
            let name = component.as_os_str();
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
            directory = match openat(&directory, name, flags, Mode::empty()) {
                Ok(fd) => File::from(fd),
                Err(rustix::io::Errno::NOENT) => {
                    match mkdirat(&directory, name, Mode::from_raw_mode(0o777)) {
                        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                        Err(error) => {
                            return Err(format!("cannot create workspace parent: {error}"));
                        }
                    }
                    File::from(openat(&directory, name, flags, Mode::empty()).map_err(|e| {
                        format!("refusing symlink or invalid workspace parent: {e}")
                    })?)
                }
                Err(error) => {
                    return Err(format!(
                        "refusing symlink or invalid workspace parent: {error}"
                    ));
                }
            };
        }
        let flags =
            OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        #[cfg(target_os = "linux")]
        let fd = rustix::fs::openat2(
            &self.root,
            &self.relative,
            flags,
            Mode::from_raw_mode(0o666),
            rustix::fs::ResolveFlags::BENEATH | rustix::fs::ResolveFlags::NO_SYMLINKS,
        );
        #[cfg(not(target_os = "linux"))]
        let fd = openat(
            &directory,
            self.relative.file_name().expect("validated leaf"),
            flags,
            Mode::from_raw_mode(0o666),
        );
        let file = File::from(
            fd.map_err(|e| format!("refusing symlink or invalid workspace write: {e}"))?,
        );
        Ok(WorkspaceFile {
            file,
            path: self.path,
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn replacement_after_preparation_cannot_redirect_file_or_parent() {
        for parent in [false, true] {
            let workspace = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("target"), "outside").unwrap();
            std::fs::create_dir(workspace.path().join("dir")).unwrap();
            std::fs::write(workspace.path().join("dir/target"), "inside").unwrap();
            let prepared = PreparedWrite::new(workspace.path(), Path::new("dir/target")).unwrap();
            if parent {
                std::fs::rename(
                    workspace.path().join("dir"),
                    workspace.path().join("parked"),
                )
                .unwrap();
                symlink(outside.path(), workspace.path().join("dir")).unwrap();
            } else {
                std::fs::remove_file(workspace.path().join("dir/target")).unwrap();
                symlink(
                    outside.path().join("target"),
                    workspace.path().join("dir/target"),
                )
                .unwrap();
            }
            assert!(prepared.open().is_err());
            assert_eq!(
                std::fs::read_to_string(outside.path().join("target")).unwrap(),
                "outside"
            );
        }
    }

    #[test]
    fn opened_handle_stays_bound_after_leaf_replacement_and_missing_parents_are_safe() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let external = outside.path().join("target");
        std::fs::write(&external, "outside").unwrap();
        let mut file = workspace_file_for_write(workspace.path(), Path::new("a/b/new")).unwrap();
        std::fs::remove_file(workspace.path().join("a/b/new")).unwrap();
        symlink(&external, workspace.path().join("a/b/new")).unwrap();
        file.write_all(b"inside").unwrap();
        assert_eq!(std::fs::read_to_string(&external).unwrap(), "outside");
        let prepared =
            PreparedWrite::new(workspace.path(), Path::new("other/missing/file")).unwrap();
        symlink(outside.path(), workspace.path().join("other")).unwrap();
        assert!(prepared.open().is_err());
        assert!(!outside.path().join("missing").exists());
    }
}

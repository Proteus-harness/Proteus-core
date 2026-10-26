//! Cheap invalidation for the inputs read by the regular configuration loader.
use std::{
    collections::BTreeMap,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Debug, PartialEq, Eq)]
enum Stamp {
    Unavailable(ErrorKind),
    Present {
        len: u64,
        modified: Option<SystemTime>,
        directory: bool,
        #[cfg(unix)]
        identity: (u64, u64, i64, i64, u32),
    },
}

impl Stamp {
    fn read(path: &Path) -> Self {
        match std::fs::metadata(path) {
            Err(error) => Self::Unavailable(error.kind()),
            Ok(metadata) => Self::Present {
                len: metadata.len(),
                modified: metadata.modified().ok(),
                directory: metadata.is_dir(),
                #[cfg(unix)]
                identity: {
                    use std::os::unix::fs::MetadataExt;
                    (
                        metadata.dev(),
                        metadata.ino(),
                        metadata.ctime(),
                        metadata.ctime_nsec(),
                        metadata.mode(),
                    )
                },
            },
        }
    }
}

/// Disabled for ordinary loads; watchers opt in without a second parser.
#[derive(Default)]
pub(crate) struct ConfigInputs {
    files: Option<BTreeMap<PathBuf, Stamp>>,
}

impl ConfigInputs {
    pub(crate) fn watching() -> Self {
        Self {
            files: Some(BTreeMap::new()),
        }
    }

    pub(crate) fn observe(&mut self, path: &Path) {
        if let Some(files) = &mut self.files {
            // Capture before reading and keep the first observation: a write
            // during parsing must invalidate the next poll, not be swallowed.
            if !files.contains_key(path) {
                files.insert(path.to_path_buf(), Stamp::read(path));
            }
        }
    }

    pub(crate) fn is_current(&self) -> bool {
        self.files.as_ref().is_some_and(|files| {
            files
                .iter()
                .all(|(path, stamp)| Stamp::read(path) == *stamp)
        })
    }

    pub(crate) fn observe_plugins(&mut self, config: &crate::domain::AddonConfig, cwd: &Path) {
        for plugin in &config.plugins {
            let path = super::expand_user_path(&plugin.path);
            let root = if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            };
            self.observe(&root);
            self.observe(&root.join("plugin.json"));
            self.observe(&root.join("mcp.json"));
        }
    }
}

#[cfg(test)]
mod tests;

//! Recently opened workspaces, newest first; offered when the client starts
//! without a workspace.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const LIMIT: usize = 12;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RecentWorkspaces(Vec<PathBuf>);

impl RecentWorkspaces {
    pub fn iter(&self) -> impl Iterator<Item = &Path> {
        self.0.iter().map(PathBuf::as_path)
    }

    /// Moves `path` to the front, dropping the oldest beyond the limit.
    pub fn touch(&mut self, path: &Path) {
        self.0.retain(|known| known != path);
        self.0.insert(0, path.to_path_buf());
        self.0.truncate(LIMIT);
    }

    pub fn forget(&mut self, path: &Path) {
        self.0.retain(|known| known != path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_moves_to_front_and_caps() {
        let mut recent = RecentWorkspaces::default();
        for index in 0..20 {
            recent.touch(Path::new(&format!("/w/{index}")));
        }
        recent.touch(Path::new("/w/15"));
        let list: Vec<_> = recent.iter().collect();
        assert_eq!(list.len(), LIMIT);
        assert_eq!(list[0], Path::new("/w/15"));
        assert_eq!(list[1], Path::new("/w/19"));
        assert_eq!(list.iter().filter(|path| **path == Path::new("/w/15")).count(), 1);
        recent.forget(Path::new("/w/15"));
        assert_eq!(recent.iter().next(), Some(Path::new("/w/19")));
    }
}

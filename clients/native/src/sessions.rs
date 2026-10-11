//! Marks the user keeps on saved sessions on this device: a custom title,
//! pinning and the archive. The server knows nothing about them; deleting a
//! session goes to the server and only forgets the marks here.

use std::collections::BTreeMap;
use std::path::Path;

use proteus_contracts::app_protocol::AppSessionSummary;
use serde::{Deserialize, Serialize};

/// Longest custom title kept; longer input is cut.
const TITLE_LIMIT: usize = 120;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionMark {
    pub title: Option<String>,
    pub pinned: bool,
    pub archived: bool,
}

impl SessionMark {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Marks by session directory.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionMarks(BTreeMap<String, SessionMark>);

fn key(dir: &Path) -> String {
    dir.to_string_lossy().into_owned()
}

impl SessionMarks {
    pub fn get(&self, dir: &Path) -> SessionMark {
        self.0.get(&key(dir)).cloned().unwrap_or_default()
    }

    fn change(&mut self, dir: &Path, change: impl FnOnce(&mut SessionMark)) {
        let key = key(dir);
        let mark = self.0.entry(key.clone()).or_default();
        change(mark);
        if mark.is_empty() {
            self.0.remove(&key);
        }
    }

    /// Sets a custom title; blank input returns to the automatic one.
    pub fn rename(&mut self, dir: &Path, title: &str) {
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        let title = (!title.is_empty()).then(|| title.chars().take(TITLE_LIMIT).collect());
        self.change(dir, |mark| mark.title = title);
    }

    pub fn set_pinned(&mut self, dir: &Path, pinned: bool) {
        self.change(dir, |mark| {
            mark.pinned = pinned;
            if pinned {
                mark.archived = false;
            }
        });
    }

    /// Archiving hides a session from the list and unpins it.
    pub fn set_archived(&mut self, dir: &Path, archived: bool) {
        self.change(dir, |mark| {
            mark.archived = archived;
            if archived {
                mark.pinned = false;
            }
        });
    }

    pub fn forget(&mut self, dir: &Path) {
        self.0.remove(&key(dir));
    }

    /// Title the user gave a session, if any.
    pub fn title(&self, dir: &Path) -> Option<&str> {
        self.0.get(&key(dir))?.title.as_deref()
    }

    /// The list as shown: pinned sessions first, then the rest in server
    /// order; archived ones only when asked for, after everything else.
    pub fn arrange<'a>(&self, sessions: &'a [AppSessionSummary], archived: bool) -> Listing<'a> {
        let mut listing = Listing::default();
        for summary in sessions {
            let mark = self.get(&summary.session_dir);
            let row = Row { summary, mark };
            if row.mark.archived {
                listing.archived_count += 1;
                if archived {
                    listing.archived.push(row);
                }
            } else if row.mark.pinned {
                listing.pinned.push(row);
            } else {
                listing.recent.push(row);
            }
        }
        listing
    }
}

pub struct Row<'a> {
    pub summary: &'a AppSessionSummary,
    pub mark: SessionMark,
}

#[derive(Default)]
pub struct Listing<'a> {
    pub pinned: Vec<Row<'a>>,
    pub recent: Vec<Row<'a>>,
    pub archived: Vec<Row<'a>>,
    /// Archived sessions, shown or not.
    pub archived_count: usize,
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use proteus_contracts::app_protocol::AppSessionSummary;

    use super::SessionMarks;

    fn summary(dir: &str) -> AppSessionSummary {
        serde_json::from_value(serde_json::json!({
            "session_dir": dir,
            "session_id": "00000000-0000-0000-0000-000000000000",
            "workspace_path": "/w",
            "message_count": 1,
            "preview": null,
        }))
        .expect("summary")
    }

    fn dirs(rows: &[super::Row<'_>]) -> Vec<PathBuf> {
        rows.iter()
            .map(|row| row.summary.session_dir.clone())
            .collect()
    }

    #[test]
    fn titles_are_trimmed_and_blank_ones_dropped() {
        let mut marks = SessionMarks::default();
        let dir = Path::new("/s/a");
        assert_eq!(marks.title(dir), None);
        marks.rename(dir, "  Сборка \n  релиза ");
        assert_eq!(marks.title(dir), Some("Сборка релиза"));
        marks.rename(dir, "   ");
        assert_eq!(marks.title(dir), None);
        assert_eq!(marks, SessionMarks::default(), "empty marks are dropped");
    }

    #[test]
    fn pinned_first_and_archive_hidden() {
        let sessions = [
            summary("/s/a"),
            summary("/s/b"),
            summary("/s/c"),
            summary("/s/d"),
        ];
        let mut marks = SessionMarks::default();
        marks.set_pinned(Path::new("/s/c"), true);
        marks.set_archived(Path::new("/s/b"), true);
        let listing = marks.arrange(&sessions, false);
        assert_eq!(dirs(&listing.pinned), [PathBuf::from("/s/c")]);
        assert_eq!(
            dirs(&listing.recent),
            [PathBuf::from("/s/a"), PathBuf::from("/s/d")]
        );
        assert!(listing.archived.is_empty());
        assert_eq!(listing.archived_count, 1);
        let listing = marks.arrange(&sessions, true);
        assert_eq!(dirs(&listing.archived), [PathBuf::from("/s/b")]);

        // Archiving unpins; pinning brings a session back from the archive.
        marks.set_archived(Path::new("/s/c"), true);
        assert!(!marks.get(Path::new("/s/c")).pinned);
        marks.set_pinned(Path::new("/s/b"), true);
        assert!(!marks.get(Path::new("/s/b")).archived);
        marks.forget(Path::new("/s/b"));
        assert_eq!(marks.get(Path::new("/s/b")), Default::default());
    }

    #[test]
    fn marks_round_trip_as_a_map() {
        let mut marks = SessionMarks::default();
        marks.set_pinned(Path::new("/s/a"), true);
        let json = serde_json::to_value(&marks).expect("json");
        assert_eq!(
            json,
            serde_json::json!({"/s/a": {"title": null, "pinned": true, "archived": false}})
        );
        assert_eq!(serde_json::from_value::<SessionMarks>(json).unwrap(), marks);
        assert!(
            serde_json::from_value::<SessionMarks>(serde_json::json!({"/s/a": {"star": true}}))
                .is_err()
        );
    }
}

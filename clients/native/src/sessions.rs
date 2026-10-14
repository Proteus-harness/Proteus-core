//! Marks the user keeps on saved sessions on this device: a custom title,
//! pinning and the archive. The server knows nothing about them; deleting a
//! session goes to the server and only forgets the marks here.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{Local, NaiveDate, TimeZone as _};
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

    /// The list as shown: pinned sessions first, then the rest by how long
    /// ago they changed, in server order within a period; archived ones
    /// only when asked for or searched, after everything else. A search
    /// keeps the sessions whose title holds it, case aside.
    pub fn arrange<'a>(&self, sessions: &'a [AppSessionSummary], view: &View) -> Listing<'a> {
        let search = view.search.trim().to_lowercase();
        let mut listing = Listing::default();
        for summary in sessions {
            let mark = self.get(&summary.session_dir);
            let row = Row { summary, mark };
            if !search.is_empty() && !row.title_holds(&search) {
                continue;
            }
            if row.mark.archived {
                listing.archived_count += 1;
                if view.archived || !search.is_empty() {
                    listing.archived.push(row);
                }
            } else if row.mark.pinned {
                listing.pinned.push(row);
            } else {
                let period = Period::of(summary.updated_at_ms.and_then(local_day), view.today);
                match listing
                    .recent
                    .iter_mut()
                    .find(|group| group.period == period)
                {
                    Some(group) => group.rows.push(row),
                    None => listing.recent.push(Group {
                        period,
                        rows: vec![row],
                    }),
                }
            }
        }
        listing.recent.sort_by_key(|group| group.period);
        listing
    }
}

/// What the list is asked to show.
pub struct View<'q> {
    pub archived: bool,
    pub search: &'q str,
    pub today: NaiveDate,
}

fn local_day(updated_at_ms: u64) -> Option<NaiveDate> {
    let ms = i64::try_from(updated_at_ms).ok()?;
    Some(Local.timestamp_millis_opt(ms).single()?.date_naive())
}

/// How long ago a session changed, as the list groups it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Period {
    Today,
    Yesterday,
    Week,
    Earlier,
}

impl Period {
    fn of(day: Option<NaiveDate>, today: NaiveDate) -> Self {
        let Some(day) = day else {
            return Self::Earlier;
        };
        match (today - day).num_days() {
            ..=0 => Self::Today,
            1 => Self::Yesterday,
            2..=6 => Self::Week,
            _ => Self::Earlier,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "Сегодня",
            Self::Yesterday => "Вчера",
            Self::Week => "На неделе",
            Self::Earlier => "Раньше",
        }
    }
}

pub struct Row<'a> {
    pub summary: &'a AppSessionSummary,
    pub mark: SessionMark,
}

impl Row<'_> {
    fn title_holds(&self, search: &str) -> bool {
        let title = self
            .mark
            .title
            .as_deref()
            .or(self.summary.preview.as_deref());
        title.is_some_and(|title| title.to_lowercase().contains(search))
    }
}

pub struct Group<'a> {
    pub period: Period,
    pub rows: Vec<Row<'a>>,
}

#[derive(Default)]
pub struct Listing<'a> {
    pub pinned: Vec<Row<'a>>,
    /// Non-empty periods, most recent first.
    pub recent: Vec<Group<'a>>,
    pub archived: Vec<Row<'a>>,
    /// Archived sessions, shown or not.
    pub archived_count: usize,
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use chrono::{Local, NaiveDate, TimeZone as _};
    use proteus_contracts::app_protocol::AppSessionSummary;

    use super::{Period, SessionMarks, View};

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 14).unwrap()
    }

    fn view(archived: bool, search: &str) -> View<'_> {
        View {
            archived,
            search,
            today: today(),
        }
    }

    fn summary(dir: &str) -> AppSessionSummary {
        changed(dir, None, None)
    }

    /// A session changed at local noon `days_ago` before [`today`].
    fn changed(dir: &str, days_ago: Option<u64>, preview: Option<&str>) -> AppSessionSummary {
        let updated_at_ms = days_ago.map(|days| {
            let day = today() - chrono::Days::new(days);
            let noon = day.and_hms_opt(12, 0, 0).unwrap();
            Local.from_local_datetime(&noon).unwrap().timestamp_millis()
        });
        serde_json::from_value(serde_json::json!({
            "session_dir": dir,
            "session_id": "00000000-0000-0000-0000-000000000000",
            "workspace_path": "/w",
            "message_count": 1,
            "preview": preview,
            "updated_at_ms": updated_at_ms,
        }))
        .expect("summary")
    }

    fn dirs<'r>(rows: impl IntoIterator<Item = &'r super::Row<'r>>) -> Vec<PathBuf> {
        rows.into_iter()
            .map(|row| row.summary.session_dir.clone())
            .collect()
    }

    /// Rows in the order they are shown.
    fn shown(listing: &super::Listing<'_>) -> Vec<PathBuf> {
        let recent = listing.recent.iter().flat_map(|group| &group.rows);
        dirs(listing.pinned.iter().chain(recent).chain(&listing.archived))
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
        let listing = marks.arrange(&sessions, &view(false, ""));
        assert_eq!(dirs(&listing.pinned), [PathBuf::from("/s/c")]);
        assert_eq!(
            shown(&listing)[1..],
            [PathBuf::from("/s/a"), PathBuf::from("/s/d")]
        );
        assert!(listing.archived.is_empty());
        assert_eq!(listing.archived_count, 1);
        let listing = marks.arrange(&sessions, &view(true, ""));
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
    fn recent_sessions_group_by_period() {
        let sessions = [
            changed("/s/old", Some(30), None),
            changed("/s/today", Some(0), None),
            changed("/s/week", Some(6), None),
            changed("/s/yesterday", Some(1), None),
            changed("/s/unknown", None, None),
            changed("/s/morning", Some(0), None),
        ];
        let listing = SessionMarks::default().arrange(&sessions, &view(false, ""));
        let periods: Vec<_> = listing.recent.iter().map(|group| group.period).collect();
        assert_eq!(
            periods,
            [
                Period::Today,
                Period::Yesterday,
                Period::Week,
                Period::Earlier
            ]
        );
        assert_eq!(
            shown(&listing),
            [
                "/s/today",
                "/s/morning",
                "/s/yesterday",
                "/s/week",
                "/s/old",
                "/s/unknown"
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn search_matches_titles_and_reaches_the_archive() {
        let sessions = [
            changed("/s/a", Some(0), Some("Почини сборку релиза")),
            changed("/s/b", Some(0), Some("Обзор тестов")),
            changed("/s/c", Some(0), None),
            changed("/s/d", Some(0), Some("Сборка")),
        ];
        let mut marks = SessionMarks::default();
        marks.rename(Path::new("/s/c"), "Ночная СБОРКА");
        marks.set_archived(Path::new("/s/d"), true);
        let listing = marks.arrange(&sessions, &view(false, "  сборк "));
        assert_eq!(shown(&listing), ["/s/a", "/s/c", "/s/d"].map(PathBuf::from));
        assert_eq!(listing.archived_count, 1);
        assert!(shown(&marks.arrange(&sessions, &view(false, "нет такого"))).is_empty());
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

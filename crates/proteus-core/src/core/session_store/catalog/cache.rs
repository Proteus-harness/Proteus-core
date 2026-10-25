//! Bounded, read-only acceleration for the UI catalog. Audit/replay never use it.
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use super::super::read_cache::{self, SourceFingerprint};
use anyhow::Result;
use proteus_contracts::app_protocol::AppSessionSummary;

const MAX_SESSIONS: usize = 512;
const MAX_FILE_STAMPS: usize = 8192;
const MAX_BLOBS: usize = 256;
const MAX_DISK_BLOBS: usize = 8192;
const DISK_NAME: &str = "summary";

struct CachedSummary {
    fingerprint: SourceFingerprint,
    summary: AppSessionSummary,
}
#[derive(Default)]
struct SummaryCache(VecDeque<CachedSummary>);
impl SummaryCache {
    fn take(&mut self, path: &Path) -> Option<CachedSummary> {
        let index = self
            .0
            .iter()
            .position(|entry| entry.summary.session_dir == path)?;
        self.0.remove(index)
    }
    fn get(&mut self, path: &Path, fingerprint: &SourceFingerprint) -> Option<AppSessionSummary> {
        let entry = self.take(path)?;
        if entry.fingerprint != *fingerprint {
            return None;
        }
        let summary = entry.summary.clone();
        self.0.push_back(entry);
        Some(summary)
    }
    fn insert(&mut self, fingerprint: SourceFingerprint, summary: AppSessionSummary) {
        self.take(&summary.session_dir);
        let mut files = self
            .0
            .iter()
            .map(|entry| entry.fingerprint.file_stamps())
            .sum::<usize>();
        let incoming = fingerprint.file_stamps();
        while self.0.len() >= MAX_SESSIONS || files + incoming > MAX_FILE_STAMPS {
            if let Some(old) = self.0.pop_front() {
                files -= old.fingerprint.file_stamps();
            }
        }
        self.0.push_back(CachedSummary {
            fingerprint,
            summary,
        });
    }
}

mod refresh;

pub(crate) fn subscribe_updates() -> tokio::sync::watch::Receiver<u64> {
    refresh::subscribe_updates()
}

pub(super) fn summary(dir: PathBuf) -> Result<AppSessionSummary> {
    let (summary, stale) =
        load_with_status(shared_cache(), dir.clone(), super::session_summary_from_dir)?;
    if stale {
        refresh::enqueue(dir);
    }
    Ok(summary)
}

fn shared_cache() -> &'static Mutex<SummaryCache> {
    static CACHE: OnceLock<Mutex<SummaryCache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Reuse an authoritative read for the catalog, without retaining its projection.
pub(super) fn prime_from_read<T>(
    dir: &Path,
    read: impl FnOnce() -> Result<(T, AppSessionSummary)>,
) -> Result<T> {
    let before = SourceFingerprint::read(dir, MAX_DISK_BLOBS);
    let (value, summary) = read()?;
    cache_if_unchanged(shared_cache(), dir, before, summary);
    Ok(value)
}

fn cache_if_unchanged(
    cache: &Mutex<SummaryCache>,
    dir: &Path,
    before: Option<SourceFingerprint>,
    summary: AppSessionSummary,
) -> bool {
    let Some(before) = before else {
        return false;
    };
    if SourceFingerprint::read(dir, MAX_DISK_BLOBS).as_ref() != Some(&before) {
        return false;
    }
    let saved = read_cache::save(dir, DISK_NAME, &before, &summary);
    if SourceFingerprint::read(dir, MAX_DISK_BLOBS).as_ref() != Some(&before) {
        return false;
    }
    if before.file_stamps() <= MAX_BLOBS + 4 {
        cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(before, summary);
    }
    saved
}

#[cfg(test)]
fn load(
    cache: &Mutex<SummaryCache>,
    dir: PathBuf,
    read: impl FnOnce(PathBuf) -> Result<AppSessionSummary>,
) -> Result<AppSessionSummary> {
    load_with_status(cache, dir, read).map(|(summary, _)| summary)
}

fn load_with_status(
    cache: &Mutex<SummaryCache>,
    dir: PathBuf,
    read: impl FnOnce(PathBuf) -> Result<AppSessionSummary>,
) -> Result<(AppSessionSummary, bool)> {
    // A removed or moved workspace must not remain visible through a cache hit.
    let workspace = super::workspace_path_from_session_dir(&dir)?;
    let valid = |summary: &AppSessionSummary| {
        summary.session_dir == dir
            && summary.workspace_path == workspace
            && super::resolve_session_identity(&dir)
                .is_ok_and(|identity| summary.session_id == identity.session_id)
            && summary.activity.is_none()
    };
    let before = SourceFingerprint::read(&dir, MAX_DISK_BLOBS);
    if let Some(fingerprint) = &before {
        let cached = cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&dir, fingerprint);
        if let Some(summary) = cached {
            if valid(&summary) {
                return Ok((summary, false));
            }
        }
    } else {
        cache.lock().unwrap_or_else(|e| e.into_inner()).take(&dir);
    }
    if let Some((source, summary, fresh)) =
        read_cache::load_any::<AppSessionSummary>(&dir, DISK_NAME)
    {
        if valid(&summary) {
            // A stale summary is only a display placeholder. Do not mark it as
            // current or retain it over a newer snapshot saved by background refresh.
            if fresh && source.file_stamps() <= MAX_BLOBS + 4 {
                cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(source, summary.clone());
            }
            return Ok((summary, !fresh));
        }
    }
    // Full validation stays outside the global cache lock. Never cache errors or
    // a snapshot whose files changed while it was being read.
    let summary = read(dir.clone())?;
    cache_if_unchanged(cache, &dir, before, summary.clone());
    Ok((summary, false))
}

#[cfg(test)]
mod tests;

//! Content-addressed pages have both row and byte budgets.
use proteus_contracts::contracts::SESSION_STORAGE_MAX_CACHE_BYTES;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{PAGE_ROWS, RemoteCacheSource, RemoteSessionStore, read_content};
use crate::core::session_store::json_size::encoded_len;

mod packs;
use packs::{LeafScratch, PackRef, PendingPack};

const NORMAL_PAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_PAGES: usize = 65_536;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::session_store::remote::view) struct PageRef {
    pub(in crate::core::session_store::remote::view) hash: String,
    pub(in crate::core::session_store::remote::view) rows: usize,
    bytes: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::session_store::remote::view) struct Section {
    pub(in crate::core::session_store::remote::view) rows: usize,
    pub(in crate::core::session_store::remote::view) pages: Vec<PageRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    packs: Vec<PackRef>,
}

fn write_pages<'a, T: Serialize + 'a>(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    rows: impl Iterator<Item = &'a T>,
    section: &mut Section,
    pack_before: usize,
) -> Option<()> {
    let mut pending = PendingPack::default();
    let mut scratch = LeafScratch::default();
    let mut page = Vec::with_capacity(PAGE_ROWS);
    let mut bytes = 2usize;
    let mut submit = |page: &[&T], bytes: usize| {
        if section.pages.len() >= MAX_PAGES {
            return None;
        }
        section.rows = section.rows.checked_add(page.len())?;
        let value = serde_json::to_value(page).ok()?;
        let hash = super::hash(&value)?;
        section.pages.push(PageRef {
            hash,
            rows: page.len(),
            bytes,
        });
        pending.push(
            store,
            source,
            section,
            value,
            section.rows <= pack_before,
            &mut scratch,
        )
    };
    for row in rows {
        let row_bytes = encoded_len(row, SESSION_STORAGE_MAX_CACHE_BYTES)?;
        if !page.is_empty()
            && (page.len() == PAGE_ROWS
                || bytes.saturating_add(row_bytes).saturating_add(1) > NORMAL_PAGE_BYTES)
        {
            submit(&page, bytes)?;
            page.clear();
            bytes = 2;
        }
        bytes = bytes
            .checked_add(row_bytes)?
            .checked_add(usize::from(!page.is_empty()))?;
        if bytes > SESSION_STORAGE_MAX_CACHE_BYTES {
            return None;
        }
        page.push(row);
    }
    if !page.is_empty() {
        submit(&page, bytes)?;
    }
    drop(scratch);
    pending.flush(store, source, section)
}

pub(in crate::core::session_store::remote::view) fn write_section<T: Serialize>(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    rows: &[T],
) -> Option<Section> {
    let mut section = Section::default();
    write_pages(store, source, rows.iter(), &mut section, 0)?;
    Some(section)
}

pub(in crate::core::session_store::remote::view) fn write_transcript<T: Serialize>(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    rows: &[T],
) -> Option<Section> {
    let mut section = Section::default();
    write_pages(
        store,
        source,
        rows.iter(),
        &mut section,
        rows.len().saturating_sub(PAGE_ROWS),
    )?;
    Some(section)
}

/// Existing complete pages are immutable; only a bounded partial tail is read.
pub(in crate::core::session_store::remote::view) fn append_section<
    T: Serialize + DeserializeOwned,
>(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    previous: &Section,
    added: &[T],
) -> Option<Section> {
    if !valid_section(previous) {
        return None;
    }
    if added.is_empty() {
        return Some(previous.clone());
    }
    let rows = previous.rows.checked_add(added.len())?;
    let mut section = previous.clone();
    let partial: Vec<T> = if section
        .pages
        .last()
        .is_some_and(|page| page.rows < PAGE_ROWS && page.bytes < NORMAL_PAGE_BYTES)
    {
        let page = section.pages.pop()?;
        section.rows -= page.rows;
        let values: Vec<T> = read_content(store, &page.hash)?;
        if values.len() != page.rows {
            return None;
        }
        values
    } else {
        vec![]
    };
    write_pages(
        store,
        source,
        partial.iter().chain(added.iter()),
        &mut section,
        rows.saturating_sub(PAGE_ROWS),
    )?;
    Some(section)
}

/// Canonical history is already available from strict validation. Its partial
/// tail can be encoded directly without reading any cached conversation body.
pub(in crate::core::session_store::remote::view) fn extend_known_section<T: Serialize>(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    previous: &Section,
    rows: &[T],
) -> Option<Section> {
    if !valid_section(previous) || rows.len() < previous.rows {
        return None;
    }
    if rows.len() == previous.rows {
        return Some(previous.clone());
    }
    let mut section = previous.clone();
    if section
        .pages
        .last()
        .is_some_and(|page| page.rows < PAGE_ROWS && page.bytes < NORMAL_PAGE_BYTES)
    {
        section.rows -= section.pages.pop()?.rows;
    }
    write_pages(store, source, rows[section.rows..].iter(), &mut section, 0)?;
    Some(section)
}

pub(in crate::core::session_store::remote::view) fn read_section<T: DeserializeOwned>(
    store: &RemoteSessionStore,
    section: &Section,
    start: usize,
    end: usize,
) -> Option<Vec<T>> {
    let mut result = None;
    visit_section(store, section, start, end, |row| {
        result
            .get_or_insert_with(|| Vec::with_capacity(end - start))
            .push(row);
        Some(())
    })?;
    Some(result.unwrap_or_default())
}

/// Only overlapping logical leaves are decoded. Each overlapping physical object is read
/// once, including when repairing the compact tool index over an entire archive.
pub(in crate::core::session_store::remote::view) fn visit_section<T: DeserializeOwned>(
    store: &RemoteSessionStore,
    section: &Section,
    start: usize,
    end: usize,
    mut visit: impl FnMut(T) -> Option<()>,
) -> Option<()> {
    if start > end || end > section.rows || !valid_section(section) {
        return None;
    }
    if start == end {
        return Some(());
    }
    let mut packs = section.packs.iter().peekable();
    let (mut index, mut offset) = (0, 0);
    while offset < end {
        let pack = packs.next_if(|pack| pack.first == index);
        let count = pack.map_or(1, |p| p.count);
        let pages = &section.pages[index..index + count];
        index += count;
        let upper = offset + pages.iter().map(|p| p.rows).sum::<usize>();
        if upper > start {
            let (encoding, bodies) = match pack {
                Some(pack) => pack.read(store, pages)?,
                None => (
                    packs::Encoding::Json,
                    vec![super::read_content_value(store, &pages[0].hash)?],
                ),
            };
            for (page, body) in pages.iter().zip(bodies) {
                let next = offset + page.rows;
                if next > start && offset < end {
                    let body = encoding.decode(body, page.bytes)?;
                    if pack.is_some() && super::hash(&body).as_deref() != Some(&page.hash) {
                        return None;
                    }
                    let rows: Vec<T> = serde_json::from_value(body).ok()?;
                    if rows.len() != page.rows {
                        return None;
                    }
                    let first = start.saturating_sub(offset);
                    let last = (end - offset).min(page.rows);
                    for value in rows.into_iter().skip(first).take(last - first) {
                        visit(value)?;
                    }
                }
                offset = next;
            }
        }
        offset = upper;
    }
    Some(())
}

pub(super) fn valid_section(section: &Section) -> bool {
    section.pages.len() <= MAX_PAGES
        && section.pages.iter().try_fold(0usize, |rows, page| {
            if !(1..=PAGE_ROWS).contains(&page.rows)
                || page.bytes > SESSION_STORAGE_MAX_CACHE_BYTES
                || (page.bytes > NORMAL_PAGE_BYTES && page.rows != 1)
                || page.hash.len() != 64
                || !page.hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return None;
            }
            rows.checked_add(page.rows)
        }) == Some(section.rows)
        && packs::valid_spans(section)
}

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

fn write_pages<T: Serialize>(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    rows: impl Iterator<Item = T>,
    section: &mut Section,
    pack_before: usize,
) -> Option<()> {
    let mut pending = PendingPack::default();
    let mut scratch = LeafScratch::default();
    let mut page = Vec::with_capacity(PAGE_ROWS);
    let mut bytes = 2usize;
    let mut submit = |page: &[T], bytes: usize| {
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
        let row_bytes = encoded_len(&row, SESSION_STORAGE_MAX_CACHE_BYTES)?;
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
    write_transcript_iter(store, source, rows.iter())
}

pub(super) fn write_transcript_iter<T: Serialize>(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    rows: impl ExactSizeIterator<Item = T>,
) -> Option<Section> {
    let mut section = Section::default();
    let pack_before = rows.len().saturating_sub(PAGE_ROWS);
    write_pages(store, source, rows, &mut section, pack_before)?;
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
    collect_section(store, section, start, end, false)
}

// Only for an immutable section of the already checked header. The
// same decoder still validates bounds, object/leaf hashes and typed rows.
pub(super) fn read_checked_transcript<T: DeserializeOwned>(
    store: &RemoteSessionStore,
    checked: &super::CheckedHeader,
    start: usize,
    end: usize,
) -> Option<Vec<T>> {
    collect_section(store, &checked.0.transcript, start, end, true)
}

impl super::CheckedSection<'_> {
    pub(super) fn read_full<T: DeserializeOwned>(
        self,
        store: &RemoteSessionStore,
    ) -> Option<Vec<T>> {
        collect_section(store, self.0, 0, self.0.rows, true)
    }

    pub(super) fn visit_full<T: DeserializeOwned>(
        self,
        store: &RemoteSessionStore,
        visit: impl FnMut(T) -> Option<()>,
    ) -> Option<()> {
        visit_section_impl(store, self.0, 0, self.0.rows, true, visit)
    }
}

fn collect_section<T: DeserializeOwned>(
    store: &RemoteSessionStore,
    section: &Section,
    start: usize,
    end: usize,
    checked: bool,
) -> Option<Vec<T>> {
    let mut result = None;
    visit_section_pages(
        store,
        section,
        start,
        end,
        checked,
        |rows: Vec<T>, selected| {
            if result.is_none()
                && selected == (0..rows.len())
                && rows.len() == end - start
                && rows.capacity() == rows.len()
            {
                // Return the exact decoded allocation without retaining spare rows.
                result = Some(rows);
            } else {
                for row in rows.into_iter().skip(selected.start).take(selected.len()) {
                    result
                        .get_or_insert_with(|| Vec::with_capacity(end - start))
                        .push(row);
                }
            }
            Some(())
        },
    )?;
    Some(result.unwrap_or_default())
}

/// Only overlapping logical leaves are decoded. Each overlapping physical object is read
/// once, including when repairing the compact tool index over an entire archive.
pub(in crate::core::session_store::remote::view) fn visit_section<T: DeserializeOwned>(
    store: &RemoteSessionStore,
    section: &Section,
    start: usize,
    end: usize,
    visit: impl FnMut(T) -> Option<()>,
) -> Option<()> {
    visit_section_impl(store, section, start, end, false, visit)
}

fn visit_section_impl<T: DeserializeOwned>(
    store: &RemoteSessionStore,
    section: &Section,
    start: usize,
    end: usize,
    checked: bool,
    mut visit: impl FnMut(T) -> Option<()>,
) -> Option<()> {
    visit_section_pages(
        store,
        section,
        start,
        end,
        checked,
        |rows: Vec<T>, selected| {
            for row in rows.into_iter().skip(selected.start).take(selected.len()) {
                visit(row)?;
            }
            Some(())
        },
    )
}

fn visit_section_pages<T: DeserializeOwned>(
    store: &RemoteSessionStore,
    section: &Section,
    start: usize,
    end: usize,
    checked: bool,
    mut visit: impl FnMut(Vec<T>, std::ops::Range<usize>) -> Option<()>,
) -> Option<()> {
    if start > end || end > section.rows || (!checked && !valid_section(section)) {
        return None;
    }
    if start == end {
        return Some(());
    }
    let (mut index, mut offset, spans) =
        if checked && start >= section.rows.saturating_sub(PAGE_ROWS) {
            // Validated pack spans cannot cover any leaf intersecting the latest
            // PAGE_ROWS. Walk backward only to the first overlapping leaf.
            let mut offset = section.rows;
            let index = section.pages.iter().rposition(|page| {
                offset -= page.rows;
                offset <= start
            })?;
            (index, offset, &[][..])
        } else {
            (0, 0, section.packs.as_slice())
        };
    let mut packs = spans.iter().peekable();
    while offset < end {
        let pack = packs.next_if(|pack| pack.first == index);
        let count = pack.map_or(1, |p| p.count);
        let pages = &section.pages[index..index + count];
        index += count;
        let upper = offset + pages.iter().map(|p| p.rows).sum::<usize>();
        if upper > start {
            let (encoding, leaf, bodies) = match pack {
                Some(pack) => {
                    let (encoding, bodies) = pack.read(store, pages)?;
                    (encoding, None, bodies)
                }
                None => (
                    packs::Encoding::Json,
                    Some(super::read_content_value(store, &pages[0].hash)?),
                    Vec::new(),
                ),
            };
            for (page, body) in pages.iter().zip(leaf.into_iter().chain(bodies)) {
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
                    visit(rows, first..last)?;
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

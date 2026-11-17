//! Content-addressed pages have both row and byte budgets.
use std::io::Write;

use proteus_contracts::contracts::SESSION_STORAGE_MAX_CACHE_BYTES;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{PAGE_ROWS, RemoteCacheSource, RemoteSessionStore, read_content, write_content};

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
}

fn encoded_len<T: Serialize>(value: &T) -> Option<usize> {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > SESSION_STORAGE_MAX_CACHE_BYTES.saturating_sub(self.0) {
                return Err(std::io::Error::other(
                    "display cache row exceeds byte budget",
                ));
            }
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.0)
}

fn write_pages<'a, T: Serialize + 'a>(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    rows: impl Iterator<Item = &'a T>,
) -> Option<Vec<PageRef>> {
    let mut pages = Vec::new();
    let mut page = Vec::with_capacity(PAGE_ROWS);
    let mut bytes = 2usize;
    for row in rows {
        let row_bytes = encoded_len(row)?;
        if !page.is_empty()
            && (page.len() == PAGE_ROWS
                || bytes.saturating_add(row_bytes).saturating_add(1) > NORMAL_PAGE_BYTES)
        {
            pages.push(PageRef {
                hash: write_content(store, source, &page)?,
                rows: page.len(),
                bytes,
            });
            if pages.len() >= MAX_PAGES {
                return None;
            }
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
        pages.push(PageRef {
            hash: write_content(store, source, &page)?,
            rows: page.len(),
            bytes,
        });
    }
    Some(pages)
}

pub(in crate::core::session_store::remote::view) fn write_section<T: Serialize>(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    rows: &[T],
) -> Option<Section> {
    let pages = write_pages(store, source, rows.iter())?;
    Some(Section {
        rows: rows.len(),
        pages,
    })
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
    let mut pages = previous.pages.clone();
    let partial: Vec<T> = if pages
        .last()
        .is_some_and(|page| page.rows < PAGE_ROWS && page.bytes < NORMAL_PAGE_BYTES)
    {
        let page = pages.pop()?;
        let values: Vec<T> = read_content(store, &page.hash)?;
        if values.len() != page.rows {
            return None;
        }
        values
    } else {
        vec![]
    };
    pages.extend(write_pages(
        store,
        source,
        partial.iter().chain(added.iter()),
    )?);
    if pages.len() > MAX_PAGES {
        return None;
    }
    Some(Section { rows, pages })
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
    let mut pages = previous.pages.clone();
    let mut start = previous.rows;
    if pages
        .last()
        .is_some_and(|page| page.rows < PAGE_ROWS && page.bytes < NORMAL_PAGE_BYTES)
    {
        start = start.checked_sub(pages.pop()?.rows)?;
    }
    pages.extend(write_pages(store, source, rows[start..].iter())?);
    if pages.len() > MAX_PAGES {
        return None;
    }
    Some(Section {
        rows: rows.len(),
        pages,
    })
}

pub(in crate::core::session_store::remote::view) fn read_section<T: DeserializeOwned>(
    store: &RemoteSessionStore,
    section: &Section,
    start: usize,
    end: usize,
) -> Option<Vec<T>> {
    if start > end || end > section.rows || !valid_section(section) {
        return None;
    }
    let mut result = Vec::with_capacity(end - start);
    if start == end {
        return Some(result);
    }
    let mut offset = 0usize;
    for page in &section.pages {
        let upper = offset.checked_add(page.rows)?;
        if upper <= start {
            offset = upper;
            continue;
        }
        if offset >= end {
            break;
        }
        let rows: Vec<T> = read_content(store, &page.hash)?;
        if rows.len() != page.rows {
            return None;
        }
        let lower = start.saturating_sub(offset);
        let count = (end - offset).min(rows.len()).checked_sub(lower)?;
        result.extend(rows.into_iter().skip(lower).take(count));
        offset = upper;
    }
    (result.len() == end - start).then_some(result)
}

pub(super) fn valid_section(section: &Section) -> bool {
    section.pages.len() <= MAX_PAGES
        && section.pages.iter().all(|page| {
            (1..=PAGE_ROWS).contains(&page.rows)
                && page.bytes <= SESSION_STORAGE_MAX_CACHE_BYTES
                && (page.bytes <= NORMAL_PAGE_BYTES || page.rows == 1)
                && page.hash.len() == 64
                && page.hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        && section
            .pages
            .iter()
            .try_fold(0usize, |rows, page| rows.checked_add(page.rows))
            == Some(section.rows)
}

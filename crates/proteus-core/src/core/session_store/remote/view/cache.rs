//! Immutable content pages with a source-bound header published last.
use std::{borrow::Cow, collections::BTreeMap};

use anyhow::{Result, bail, ensure};
use proteus_contracts::{
    app_protocol::{AppLatestToolState, AppToolState, AppTranscriptBase},
    contracts::{
        ModelContextObservation, SessionStorageRequest as Request,
        SessionStorageResponse as Response, StorageSnapshot,
    },
    model_standard::CanonicalMessage,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::super::plain_history::PlainHistoryMessage;
use super::*;
use crate::core::session_store::remote::snapshot::RemoteJournalCursor;
pub(super) use preview::{plain_preview, session_preview};

const FORMAT: u32 = 1;
pub(super) const PAGE_ROWS: usize = 50;
const HEAD: &str = "display-view-v1-head";
const PREFIX: &str = "display-view-v1-prefix";

mod pages;
mod preview;
pub(super) use pages::{
    Section, append_section, extend_known_section, read_section, visit_section, write_section,
    write_transcript,
};
use pages::{read_checked_transcript, valid_section, write_transcript_iter};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteCacheSource {
    pub(crate) snapshot: StorageSnapshot,
}

impl PartialEq for RemoteCacheSource {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot.session == other.snapshot.session
            && self.snapshot.revision == other.snapshot.revision
            && self.snapshot.content_sha256 == other.snapshot.content_sha256
    }
}
impl Eq for RemoteCacheSource {}

impl RemoteCacheSource {
    pub(crate) fn read(store: &RemoteSessionStore) -> Option<Self> {
        Self::try_read(store).ok()
    }
    pub(crate) fn try_read(store: &RemoteSessionStore) -> Result<Self> {
        Ok(Self {
            snapshot: store.journal_snapshot()?,
        })
    }
    pub(crate) fn from_cursor(cursor: &RemoteJournalCursor) -> Self {
        Self {
            snapshot: cursor.snapshot.clone(),
        }
    }
    pub(crate) fn revision(&self) -> Option<String> {
        hash(
            &serde_json::to_value((
                &self.snapshot.session,
                self.snapshot.revision,
                &self.snapshot.content_sha256,
                FORMAT,
            ))
            .ok()?,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Header {
    pub(super) format: u32,
    pub(super) source: RemoteCacheSource,
    pub(super) summary: AppSessionSummary,
    pub(super) revision: u64,
    pub(super) usage: SessionUsageSnapshot,
    pub(super) context_summary: AppContextHistorySummary,
    pub(super) conversation: Section,
    pub(super) transcript: Section,
    pub(super) context: Section,
    pub(super) cursor: Option<RemoteJournalCursor>,
    pub(super) base: Option<AppTranscriptBase>,
    pub(super) tools: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) prefix: Option<String>,
}

fn hash(value: &Value) -> Option<String> {
    proteus_contracts::contracts::storage_value_etag(value).ok()
}

fn read_value(store: &RemoteSessionStore, key: &str) -> Option<Value> {
    match store
        .request(Request::ReadCache {
            session: store.descriptor.session.clone(),
            key: key.to_owned(),
            source: None,
        })
        .ok()?
    {
        Response::Metadata {
            document: Some(document),
        } => Some(document.value),
        _ => None,
    }
}

fn write_value(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    key: &str,
    value: Value,
) -> Option<()> {
    match store
        .request(Request::WriteCache {
            session: store.descriptor.session.clone(),
            key: key.to_owned(),
            source: source.snapshot.clone(),
            value,
        })
        .ok()?
    {
        Response::CacheWritten { etag: Some(_) } => Some(()),
        _ => None,
    }
}

pub(super) fn read_content<T: DeserializeOwned>(
    store: &RemoteSessionStore,
    expected: &str,
) -> Option<T> {
    serde_json::from_value(read_content_value(store, expected)?).ok()
}

fn read_content_value(store: &RemoteSessionStore, expected: &str) -> Option<Value> {
    let value = read_value(store, &format!("display-data-{expected}"))?;
    (hash(&value).as_deref() == Some(expected)).then_some(value)
}

pub(super) fn read_prefix(store: &RemoteSessionStore, expected: &str) -> Option<PrefixIdentities> {
    let value = read_value(store, PREFIX)?;
    (hash(&value).as_deref() == Some(expected)).then_some(())?;
    PrefixIdentities::decode_cache(value)
}

pub(super) fn write_prefix(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    prefix: &PrefixIdentities,
) -> Option<String> {
    // Only the latest projection appends to this index. Keep one copy even
    // when old revisions still serve their immutable display pages.
    write_prefix_value(store, source, prefix.encode_cache()?.into_bounded_value()?)
}

fn write_prefix_value(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    value: Value,
) -> Option<String> {
    let digest = hash(&value)?;
    write_value(store, source, PREFIX, value)?;
    Some(digest)
}

fn bounded_value(value: &impl Serialize) -> Option<Value> {
    // Reject oversized groups before building an owned Value copy. The common
    // path serializes at most one display page; larger caches remain misses.
    crate::core::session_store::json_size::encoded_len(
        value,
        proteus_contracts::contracts::SESSION_STORAGE_MAX_CACHE_BYTES,
    )?;
    serde_json::to_value(value).ok()
}

pub(super) fn write_content<T: Serialize>(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    value: &T,
) -> Option<String> {
    let value = bounded_value(value)?;
    let digest = hash(&value)?;
    write_content_value(store, source, &digest, value)?;
    Some(digest)
}

fn write_content_value(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    digest: &str,
    value: Value,
) -> Option<()> {
    let key = format!("display-data-{digest}");
    // Unchanged payload pages remain in the backend; only new content is sent.
    if read_value(store, &key).is_some_and(|existing| hash(&existing).as_deref() == Some(digest)) {
        return Some(());
    }
    write_value(store, source, &key, value)
}

fn matches(store: &RemoteSessionStore, header: &Header) -> bool {
    header.format == FORMAT
        && header.source.snapshot.session == store.descriptor.session
        && header.summary.session_id == store.session_id()
        && header.summary.parent_session_id == store.parent_session_id()
        && header.summary.session_dir == store.session_dir()
        && header.summary.workspace_path == store.descriptor.workspace_path
        && header.summary.message_count == header.conversation.rows
        && header.revision == header.source.snapshot.revision.history_revision
        && header.usage.session_id == store.session_id()
        && [&header.conversation, &header.transcript, &header.context]
            .into_iter()
            .all(valid_section)
        && header.tools.len() <= 16_384
}

// Paging may use a section without rescanning it, but only after the
// complete source and section checks on this owned, immutable header.
struct CheckedHeader(Header);

impl CheckedHeader {
    fn read(store: &RemoteSessionStore, key: &str) -> Option<Self> {
        let header = serde_json::from_value(read_value(store, key)?).ok()?;
        matches(store, &header).then_some(Self(header))
    }
}

pub(super) fn header(store: &RemoteSessionStore) -> Option<Header> {
    CheckedHeader::read(store, HEAD).map(|checked| checked.0)
}

pub(super) fn header_at_revision(store: &RemoteSessionStore, revision: &str) -> Option<Header> {
    checked_header_at_revision(store, revision).map(|checked| checked.0)
}

fn checked_header_at_revision(store: &RemoteSessionStore, revision: &str) -> Option<CheckedHeader> {
    if revision.len() != 64 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let header = CheckedHeader::read(store, &format!("display-revision-{revision}"))?;
    (header.0.source.revision().as_deref() == Some(revision)).then_some(header)
}

pub(super) fn fresh_header(store: &RemoteSessionStore) -> Option<Header> {
    let header = header(store)?;
    (RemoteCacheSource::read(store).as_ref() == Some(&header.source)).then_some(header)
}

pub(super) fn publish(store: &RemoteSessionStore, header: &Header) -> Option<()> {
    if !matches(store, header) {
        return None;
    }
    let revision = header.source.revision()?;
    write_value(
        store,
        &header.source,
        &format!("display-revision-{revision}"),
        serde_json::to_value(header).ok()?,
    )?;
    // Readers only use sections referenced by the final committed header.
    write_value(
        store,
        &header.source,
        HEAD,
        serde_json::to_value(header).ok()?,
    )
}

fn load_cached<T>(
    store: &RemoteSessionStore,
    read_conversation: impl FnOnce(&Header) -> Option<(T, usize)>,
) -> Option<(Header, T, Vec<AppTranscriptMessage>)> {
    let header = fresh_header(store)?;
    let (conversation, message_count) = read_conversation(&header)?;
    let transcript = read_section(store, &header.transcript, 0, header.transcript.rows)?;
    (SessionReadModel::matches_parts(store, &header.summary, &header.usage, message_count)
        && RemoteCacheSource::read(store).as_ref() == Some(&header.source))
    .then_some((header, conversation, transcript))
}

pub(super) fn load_model(store: &RemoteSessionStore) -> Option<SessionReadModel> {
    let (header, conversation, transcript) = load_cached(store, |header| {
        let messages: Vec<CanonicalMessage> =
            read_section(store, &header.conversation, 0, header.conversation.rows)?;
        let message_count = messages.len();
        let model_context = read_section(store, &header.context, 0, header.context.rows)?;
        Some((
            ConversationSnapshot {
                revision: header.revision,
                messages,
                model_context,
            },
            message_count,
        ))
    })?;
    Some(SessionReadModel {
        summary: header.summary,
        conversation,
        transcript,
        usage: header.usage,
        prefix: None,
    })
}

pub(super) fn load_transcript(store: &RemoteSessionStore) -> Option<Vec<AppTranscriptMessage>> {
    load_cached(store, |header| {
        // Verify the same typed rows and pages, but keep neither unused output vector.
        let mut message_count = 0usize;
        visit_section::<CanonicalMessage>(
            store,
            &header.conversation,
            0,
            header.conversation.rows,
            |_| {
                message_count = message_count.checked_add(1)?;
                Some(())
            },
        )?;
        visit_section::<ModelContextObservation>(
            store,
            &header.context,
            0,
            header.context.rows,
            |_| Some(()),
        )?;
        Some(((), message_count))
    })
    .map(|(_, _, transcript)| transcript)
}

pub(super) fn load_view(
    store: &RemoteSessionStore,
) -> Option<(RemoteCacheSource, SessionReadView, bool)> {
    let (source, view) = load_saved_view(store)?;
    let fresh = RemoteCacheSource::read(store).as_ref() == Some(&source);
    Some((source, view, fresh))
}

pub(super) fn load_saved_view(
    store: &RemoteSessionStore,
) -> Option<(RemoteCacheSource, SessionReadView)> {
    let checked = CheckedHeader::read(store, HEAD)?;
    let end = checked.0.transcript.rows;
    let start = end.saturating_sub(PAGE_ROWS);
    let transcript = read_checked_transcript(store, &checked, start, end)?;
    let CheckedHeader(header) = checked;
    let view = SessionReadView {
        summary: header.summary,
        revision: header.revision,
        usage: header.usage,
        transcript,
        cursor: Some(AppTranscriptCursor {
            revision: header.source.revision()?,
            before: start,
            base: header.base,
        }),
    };
    Some((header.source, view))
}

pub(super) fn transcript_page(
    store: &RemoteSessionStore,
    before: Option<usize>,
    expected: Option<&str>,
    stale_display: bool,
) -> Option<AppTranscriptPage> {
    if let Some(revision) = expected {
        // A retained, explicitly named revision can be older than the journal.
        return page_from_header(store, checked_header_at_revision(store, revision)?, before);
    }
    let head = CheckedHeader::read(store, HEAD)?;
    if stale_display {
        return page_from_header(store, head, before);
    }
    // All page content is addressed by hash. Check the complete source identity
    // AFTER the read: an earlier probe adds no proof to a successful final check.
    let source = head.0.source.clone();
    let page = page_from_header(store, head, before);
    let current = RemoteCacheSource::read(store)?;
    if source == current {
        return page;
    }
    drop(page);
    // A publisher stores the named index before HEAD. If the last write failed,
    // a complete index for the current source may survive behind an older HEAD.
    // This remains a latest-page request; recheck after reading that index too.
    let saved = checked_header_at_revision(store, &current.revision()?)?;
    if saved.0.source != current {
        return None;
    }
    let page = page_from_header(store, saved, before)?;
    (RemoteCacheSource::read(store)? == current).then_some(page)
}

fn page_from_header(
    store: &RemoteSessionStore,
    checked: CheckedHeader,
    before: Option<usize>,
) -> Option<AppTranscriptPage> {
    let end = before.unwrap_or(checked.0.transcript.rows);
    if end > checked.0.transcript.rows {
        return None;
    }
    let start = end.saturating_sub(PAGE_ROWS);
    let messages = read_checked_transcript(store, &checked, start, end)?;
    let CheckedHeader(header) = checked;
    Some(AppTranscriptPage {
        revision: header.source.revision()?,
        start,
        total: header.transcript.rows,
        messages,
        base: header.base,
    })
}

pub(super) fn save_model(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    model: &SessionReadModel,
    cursor: Option<&RemoteJournalCursor>,
) -> Option<()> {
    save_model_sections(
        store,
        source,
        ReadMetadata {
            summary: &model.summary,
            revision: model.conversation.revision,
            usage: &model.usage,
        },
        cursor,
        CacheConversation::Borrowed(&model.conversation.messages),
        Cow::Borrowed(&model.conversation.model_context),
        RowsRef::Full(&model.transcript),
        model.prefix.as_ref().map(CachePrefix::Borrowed),
    )
}

/// A startup projection holds only its returned display row owner and a
/// disposable independent canonical conversation until that section saves.
pub(super) struct StartupProjection<'a, T> {
    pub(super) summary: &'a AppSessionSummary,
    pub(super) revision: u64,
    pub(super) usage: &'a SessionUsageSnapshot,
    pub(super) history: StartupHistory,
    pub(super) context: Vec<ModelContextObservation>,
    pub(super) transcript: &'a T,
    pub(super) prefix: Option<PrefixIdentities>,
}

pub(super) fn save_startup_projection<T: CacheRows>(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    model: StartupProjection<'_, T>,
) -> Option<()> {
    save_model_sections(
        store,
        source,
        ReadMetadata {
            summary: model.summary,
            revision: model.revision,
            usage: model.usage,
        },
        None,
        CacheConversation::Owned(model.history),
        Cow::Owned(model.context),
        model.transcript.rows_ref(),
        model.prefix.map(CachePrefix::Owned),
    )
}

#[cfg(test)]
pub(super) fn save_startup_model<T: CacheRows>(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    model: &mut SessionReadModel<T>,
) -> Option<()> {
    save_startup_projection(
        store,
        source,
        StartupProjection {
            summary: &model.summary,
            revision: model.conversation.revision,
            usage: &model.usage,
            history: StartupHistory::Full(std::mem::take(&mut model.conversation.messages)),
            context: std::mem::take(&mut model.conversation.model_context),
            transcript: &model.transcript,
            prefix: model.prefix.take(),
        },
    )
}

pub(super) fn write_plain_conversation(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    rows: &[PlainHistoryMessage],
) -> Option<Section> {
    write_transcript_iter(store, source, rows.iter())
}

#[derive(Clone, Copy)]
pub(super) enum RowsRef<'a> {
    Full(&'a [AppTranscriptMessage]),
    Text {
        timestamp_ms: i64,
        rows: &'a [CompactTextRow],
    },
}

pub(super) trait CacheRows {
    fn rows_ref(&self) -> RowsRef<'_>;
}

impl CacheRows for Vec<AppTranscriptMessage> {
    fn rows_ref(&self) -> RowsRef<'_> {
        RowsRef::Full(self)
    }
}

impl CacheRows for DisplayRows {
    fn rows_ref(&self) -> RowsRef<'_> {
        match self {
            DisplayRows::Full(rows) => RowsRef::Full(rows),
            DisplayRows::Text { timestamp_ms, rows } => RowsRef::Text {
                timestamp_ms: *timestamp_ms,
                rows,
            },
        }
    }
}

enum CachePrefix<'a> {
    Borrowed(&'a PrefixIdentities),
    Owned(PrefixIdentities),
}

impl CachePrefix<'_> {
    fn write(self, store: &RemoteSessionStore, source: &RemoteCacheSource) -> Option<String> {
        match self {
            Self::Borrowed(prefix) => write_prefix(store, source, prefix),
            Self::Owned(prefix) => {
                let value = prefix.into_encoded_cache()?.into_bounded_value()?;
                write_prefix_value(store, source, value)
            }
        }
    }
}

struct ReadMetadata<'a> {
    summary: &'a AppSessionSummary,
    revision: u64,
    usage: &'a SessionUsageSnapshot,
}

enum CacheConversation<'a> {
    Borrowed(&'a [CanonicalMessage]),
    Owned(StartupHistory),
}

impl CacheConversation<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Borrowed(rows) => rows.len(),
            Self::Owned(history) => history.len(),
        }
    }

    fn write(&self, store: &RemoteSessionStore, source: &RemoteCacheSource) -> Option<Section> {
        match self {
            Self::Borrowed(rows) => write_transcript(store, source, rows),
            Self::Owned(history) => history.write(store, source),
        }
    }
}

enum SavedContextSummary<'a> {
    Borrowed(&'a [CanonicalMessage]),
    Computed(AppContextHistorySummary),
}

impl SavedContextSummary<'_> {
    fn finish(self) -> AppContextHistorySummary {
        match self {
            Self::Borrowed(rows) => crate::core::summarize_context_history(rows),
            Self::Computed(summary) => summary,
        }
    }
}

fn save_model_sections(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    metadata: ReadMetadata<'_>,
    cursor: Option<&RemoteJournalCursor>,
    messages: CacheConversation<'_>,
    observations: Cow<'_, [ModelContextObservation]>,
    rows: RowsRef<'_>,
    prefix: Option<CachePrefix<'_>>,
) -> Option<()> {
    if !SessionReadModel::matches_parts(store, metadata.summary, metadata.usage, messages.len()) {
        return None;
    }
    // Only the prior source and transcript participate in prefix comparison.
    let previous = header(store).map(|header| (header.source, header.transcript));
    let conversation = messages.write(store, source)?;
    let context_summary = match messages {
        CacheConversation::Owned(history) => {
            // Drop all canonical owners before allocating the display section.
            SavedContextSummary::Computed(history.context_summary())
        }
        CacheConversation::Borrowed(messages) => SavedContextSummary::Borrowed(messages),
    };
    let transcript = match rows {
        RowsRef::Full(rows) => write_transcript(store, source, rows)?,
        RowsRef::Text { timestamp_ms, rows } => {
            write_transcript_iter(store, source, rows.iter().map(|row| row.wire(timestamp_ms)))?
        }
    };
    let context = write_section(store, source, observations.as_ref())?;
    drop(observations);
    let base = previous.and_then(|(old_source, old_transcript)| {
        Some(AppTranscriptBase {
            revision: old_source.revision()?,
            common_prefix: match rows {
                RowsRef::Full(rows) => {
                    common_prefix(store, &old_transcript, &transcript, rows.len(), |i| {
                        rows.get(i)
                    })?
                }
                RowsRef::Text { timestamp_ms, rows } => {
                    common_prefix(store, &old_transcript, &transcript, rows.len(), |i| {
                        rows.get(i).map(|row| row.wire(timestamp_ms))
                    })?
                }
            },
        })
    });
    let mut tools = BTreeMap::new();
    if let RowsRef::Full(rows) = rows {
        update_tools(store, source, &mut tools, rows)?;
    }
    publish(
        store,
        &Header {
            format: FORMAT,
            source: source.clone(),
            summary: metadata.summary.clone(),
            revision: metadata.revision,
            usage: metadata.usage.clone(),
            context_summary: context_summary.finish(),
            conversation,
            transcript,
            context,
            cursor: cursor.cloned(),
            base,
            tools,
            prefix: prefix.and_then(|index| index.write(store, source)),
        },
    )
}

fn common_prefix<T: Serialize>(
    store: &RemoteSessionStore,
    old: &Section,
    new: &Section,
    len: usize,
    row: impl Fn(usize) -> Option<T>,
) -> Option<usize> {
    let mut common = 0;
    for (before, after) in old.pages.iter().zip(&new.pages) {
        if before.hash == after.hash && before.rows == after.rows {
            common += before.rows;
            continue;
        }
        let old_rows: Vec<Value> =
            read_section(store, old, common, common.checked_add(before.rows)?)?;
        for old_row in &old_rows {
            let Some(new_row) = row(common) else {
                return Some(common);
            };
            if hash(old_row)? != hash(&serde_json::to_value(new_row).ok()?)? {
                return Some(common);
            }
            common += 1;
        }
        return Some(common.min(old.rows).min(len));
    }
    Some(common.min(old.rows).min(len))
}

pub(super) fn update_tools(
    store: &RemoteSessionStore,
    source: &RemoteCacheSource,
    tools: &mut BTreeMap<String, String>,
    rows: &[AppTranscriptMessage],
) -> Option<()> {
    for row in rows {
        if let Some(tool) = &row.tool
            && accepted(&tool.status)
        {
            tools.insert(
                tool.name.clone(),
                write_content(
                    store,
                    source,
                    &CompactTool {
                        call_id: tool.call_id.as_str(),
                        name: tool.name.as_str(),
                        status: tool.status.as_str(),
                        args: &tool.args,
                        effective_args: tool.effective_args.as_ref(),
                    },
                )?,
            );
        }
    }
    (tools.len() <= 16_384).then_some(())
}

fn accepted(status: &str) -> bool {
    !matches!(status, "failed" | "denied")
}

pub(super) fn tool_state(store: &RemoteSessionStore, name: &str) -> Option<AppLatestToolState> {
    let header = fresh_header(store)?;
    let tool = match header.tools.get(name) {
        Some(digest) => match read_content::<AppToolState>(store, digest) {
            Some(tool) => Some(tool),
            None => return rebuild_tools(store, header, name),
        },
        None => None,
    };
    if tool
        .as_ref()
        .is_some_and(|tool| tool.name != name || !accepted(&tool.status))
    {
        return None;
    }
    (RemoteCacheSource::read(store).as_ref() == Some(&header.source)).then(|| {
        Some(AppLatestToolState {
            revision: header.source.revision()?,
            tool,
        })
    })?
}

// Ignore full text/output/image fields while rebuilding a disposable compact
// tool index. Each source page is independently verified before decoding it.
#[derive(Deserialize)]
struct CompactRow {
    tool: Option<CompactTool>,
}
// Mirrors the AppToolState wire shape. Borrow for writes; own fields on rebuild.
#[derive(Serialize, Deserialize)]
struct CompactTool<S = String, A = Value> {
    call_id: S,
    name: S,
    status: S,
    args: A,
    effective_args: Option<A>,
}

fn rebuild_tools(
    store: &RemoteSessionStore,
    mut header: Header,
    name: &str,
) -> Option<AppLatestToolState> {
    let mut tools = BTreeMap::new();
    let mut requested = None;
    visit_section(
        store,
        &header.transcript,
        0,
        header.transcript.rows,
        |row: CompactRow| {
            if let Some(tool) = row.tool
                && accepted(&tool.status)
            {
                let state = AppToolState {
                    call_id: tool.call_id,
                    name: tool.name,
                    status: tool.status,
                    args: tool.args,
                    effective_args: tool.effective_args,
                };
                let is_requested = state.name == name;
                if is_requested {
                    // Release the previous match before serializing its replacement.
                    drop(requested.take());
                }
                tools.insert(
                    state.name.clone(),
                    write_content(store, &header.source, &state)?,
                );
                if is_requested {
                    requested = Some(state);
                }
            }
            Some(())
        },
    )?;
    if RemoteCacheSource::read(store).as_ref() != Some(&header.source) {
        return None;
    }
    header.tools = tools;
    let _ = publish(store, &header);
    Some(AppLatestToolState {
        revision: header.source.revision()?,
        tool: requested,
    })
}

pub(super) fn summary(
    store: &RemoteSessionStore,
    messages: &[CanonicalMessage],
) -> Result<AppSessionSummary> {
    summary_with(store, messages.len(), || session_preview(messages))
}

pub(super) fn summary_with(
    store: &RemoteSessionStore,
    message_count: usize,
    make_preview: impl FnOnce() -> Option<String>,
) -> Result<AppSessionSummary> {
    let descriptor = match store.request(Request::Describe {
        session: store.descriptor.session.clone(),
    })? {
        Response::Session { descriptor } => descriptor,
        _ => bail!("storage returned an unexpected descriptor"),
    };
    ensure!(
        descriptor.session == store.descriptor.session,
        "storage summary belongs to another session"
    );
    let mut summary = AppSessionSummary::new(
        store.session_dir().to_owned(),
        store.session_id(),
        descriptor.workspace_path,
        message_count,
        u64::try_from(descriptor.updated_at_ms).ok(),
        make_preview(),
    );
    summary.parent_session_id = store.parent_session_id();
    Ok(summary)
}

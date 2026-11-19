//! Immutable content pages with a source-bound header published last.
use std::{collections::BTreeMap, io::Write};

use anyhow::{Result, bail, ensure};
use proteus_contracts::{
    app_protocol::{AppLatestToolState, AppToolState, AppTranscriptBase},
    contracts::{
        SessionStorageRequest as Request, SessionStorageResponse as Response, StorageSnapshot,
    },
    model_standard::{CanonicalMessage, ContentPart, MessageRole},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::*;
use crate::core::session_store::remote::snapshot::RemoteJournalCursor;

const FORMAT: u32 = 1;
pub(super) const PAGE_ROWS: usize = 50;
const HEAD: &str = "display-view-v1-head";
const PREFIX: &str = "display-view-v1-prefix";

mod pages;
use pages::valid_section;
pub(super) use pages::{
    Section, append_section, extend_known_section, read_section, visit_section, write_section,
    write_transcript,
};

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
    let value = bounded_value(&prefix.encode_cache()?)?;
    let digest = hash(&value)?;
    write_value(store, source, PREFIX, value)?;
    Some(digest)
}

fn bounded_value(value: &impl Serialize) -> Option<Value> {
    // Reject oversized groups before building an owned Value copy. The common
    // path serializes at most one display page; larger caches remain misses.
    struct Limit(usize);
    impl Write for Limit {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len()
                > proteus_contracts::contracts::SESSION_STORAGE_MAX_CACHE_BYTES
                    .saturating_sub(self.0)
            {
                return Err(std::io::Error::other(
                    "display cache page exceeds byte budget",
                ));
            }
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(&mut Limit(0), value).ok()?;
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

pub(super) fn header(store: &RemoteSessionStore) -> Option<Header> {
    let header = serde_json::from_value(read_value(store, HEAD)?).ok()?;
    matches(store, &header).then_some(header)
}

pub(super) fn header_at_revision(store: &RemoteSessionStore, revision: &str) -> Option<Header> {
    if revision.len() != 64 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let header =
        serde_json::from_value(read_value(store, &format!("display-revision-{revision}"))?).ok()?;
    (matches(store, &header) && header.source.revision().as_deref() == Some(revision))
        .then_some(header)
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

pub(super) fn load_model(store: &RemoteSessionStore) -> Option<SessionReadModel> {
    let header = fresh_header(store)?;
    let model = SessionReadModel {
        summary: header.summary,
        conversation: ConversationSnapshot {
            revision: header.revision,
            messages: read_section(store, &header.conversation, 0, header.conversation.rows)?,
            model_context: read_section(store, &header.context, 0, header.context.rows)?,
        },
        transcript: read_section(store, &header.transcript, 0, header.transcript.rows)?,
        usage: header.usage,
        prefix: None,
    };
    (model.matches(store) && RemoteCacheSource::read(store).as_ref() == Some(&header.source))
        .then_some(model)
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
    let header = header(store)?;
    let start = header.transcript.rows.saturating_sub(PAGE_ROWS);
    let transcript = read_section(store, &header.transcript, start, header.transcript.rows)?;
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
    let header = match expected {
        Some(revision) => header_at_revision(store, revision)?,
        None => header(store)?,
    };
    if expected.is_none()
        && !stale_display
        && RemoteCacheSource::read(store).as_ref() != Some(&header.source)
    {
        return None;
    }
    let end = before.unwrap_or(header.transcript.rows);
    if end > header.transcript.rows {
        return None;
    }
    let start = end.saturating_sub(PAGE_ROWS);
    let messages = read_section(store, &header.transcript, start, end)?;
    if expected.is_none()
        && !stale_display
        && RemoteCacheSource::read(store).as_ref() != Some(&header.source)
    {
        return None;
    }
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
    if !model.matches(store) {
        return None;
    }
    let previous = header(store);
    let conversation = write_section(store, source, &model.conversation.messages)?;
    let transcript = write_transcript(store, source, &model.transcript)?;
    let context = write_section(store, source, &model.conversation.model_context)?;
    let base = previous.as_ref().and_then(|previous| {
        Some(AppTranscriptBase {
            revision: previous.source.revision()?,
            common_prefix: common_prefix(
                store,
                &previous.transcript,
                &transcript,
                &model.transcript,
            )?,
        })
    });
    let mut tools = BTreeMap::new();
    update_tools(store, source, &mut tools, &model.transcript)?;
    publish(
        store,
        &Header {
            format: FORMAT,
            source: source.clone(),
            summary: model.summary.clone(),
            revision: model.conversation.revision,
            usage: model.usage.clone(),
            context_summary: crate::core::summarize_context_history(&model.conversation.messages),
            conversation,
            transcript,
            context,
            cursor: cursor.cloned(),
            base,
            tools,
            prefix: model
                .prefix
                .as_ref()
                .and_then(|index| write_prefix(store, source, index)),
        },
    )
}

fn common_prefix(
    store: &RemoteSessionStore,
    old: &Section,
    new: &Section,
    rows: &[AppTranscriptMessage],
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
            let Some(new_row) = rows.get(common) else {
                return Some(common);
            };
            if hash(old_row)? != hash(&serde_json::to_value(new_row).ok()?)? {
                return Some(common);
            }
            common += 1;
        }
        return Some(common.min(old.rows).min(rows.len()));
    }
    Some(common.min(old.rows).min(rows.len()))
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
                write_content(store, source, &AppToolState::from(tool))?,
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
#[derive(Deserialize)]
struct CompactTool {
    call_id: String,
    name: String,
    status: String,
    args: Value,
    effective_args: Option<Value>,
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
                if state.name == name {
                    requested = Some(state.clone());
                }
                tools.insert(
                    state.name.clone(),
                    write_content(store, &header.source, &state)?,
                );
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
    let preview = messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .chain(messages.iter())
        .find_map(|message| {
            message.parts.iter().find_map(|part| {
                let text = match &part.payload {
                    ContentPart::Text { text }
                    | ContentPart::Reasoning { text, .. }
                    | ContentPart::ReasoningSummary { text } => text.trim().to_owned(),
                    ContentPart::ToolResult { result } => result.text_or_status().trim().to_owned(),
                    _ => return None,
                };
                if text.is_empty() {
                    None
                } else {
                    Some(if text.chars().count() > 160 {
                        format!("{}...", text.chars().take(160).collect::<String>())
                    } else {
                        text
                    })
                }
            })
        });
    let mut summary = AppSessionSummary::new(
        store.session_dir().to_owned(),
        store.session_id(),
        descriptor.workspace_path,
        messages.len(),
        u64::try_from(descriptor.updated_at_ms).ok(),
        preview,
    );
    summary.parent_session_id = store.parent_session_id();
    Ok(summary)
}

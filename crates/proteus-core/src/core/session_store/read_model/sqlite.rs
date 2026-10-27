//! Sections are decoded independently; text rows are fetched only when requested.
use super::*;
use crate::core::session_store::read_cache::{Reader, SourceFingerprint};

mod save;
pub(super) use save::save;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    summary: AppSessionSummary,
    revision: u64,
    conversation_len: usize,
    transcript_len: usize,
    context_len: usize,
    context_summary: proteus_contracts::app_protocol::AppContextHistorySummary,
    base: Option<proteus_contracts::app_protocol::AppTranscriptBase>,
}

pub(super) fn view(store: &SessionStore) -> Option<(SourceFingerprint, SessionReadView, bool)> {
    let reader = Reader::open(store.session_dir(), VIEW_CACHE)?;
    let header: Header = reader.one("header")?;
    if !matches(store, &header) {
        return None;
    }
    let start = header.transcript_len.saturating_sub(50);
    let transcript = reader.page("transcript", start, header.transcript_len)?;
    let usage: SessionUsageSnapshot = reader.one("usage")?;
    if usage.session_id != store.session_id() {
        return None;
    }
    let (source, fresh) = reader.finish()?;
    let revision = source.revision()?;
    Some((
        source,
        SessionReadView {
            summary: header.summary,
            revision: header.revision,
            usage,
            transcript,
            cursor: Some(proteus_contracts::app_protocol::AppTranscriptCursor {
                revision,
                before: start,
                base: header.base,
            }),
        },
        fresh,
    ))
}

pub(in crate::core::session_store) fn conversation(
    store: &SessionStore,
) -> Option<ConversationSnapshot> {
    let reader = Reader::open(store.session_dir(), VIEW_CACHE)?;
    let header: Header = reader.one("header")?;
    if !matches(store, &header) {
        return None;
    }
    let snapshot = ConversationSnapshot {
        revision: header.revision,
        messages: reader.page("conversation", 0, header.conversation_len)?,
        model_context: reader.page("context", 0, header.context_len)?,
    };
    let (_, fresh) = reader.finish()?;
    fresh.then_some(snapshot)
}

pub(super) fn load(store: &SessionStore) -> Option<(SourceFingerprint, SessionReadModel, bool)> {
    let reader = Reader::open(store.session_dir(), VIEW_CACHE)?;
    let header: Header = reader.one("header")?;
    if !matches(store, &header) {
        return None;
    }
    let model = SessionReadModel {
        summary: header.summary,
        conversation: ConversationSnapshot {
            revision: header.revision,
            messages: reader.page("conversation", 0, header.conversation_len)?,
            model_context: reader.page("context", 0, header.context_len)?,
        },
        transcript: reader.page("transcript", 0, header.transcript_len)?,
        usage: reader.one("usage")?,
    };
    if !model.matches(store) {
        return None;
    }
    let (source, fresh) = reader.finish()?;
    Some((source, model, fresh))
}

pub(super) fn usage(store: &SessionStore) -> Option<SessionUsageSnapshot> {
    let reader = Reader::open(store.session_dir(), VIEW_CACHE)?;
    let header: Header = reader.one("header")?;
    if !matches(store, &header) {
        return None;
    }
    let usage: SessionUsageSnapshot = reader.one("usage")?;
    if usage.session_id != store.session_id() {
        return None;
    }
    let (_, fresh) = reader.finish()?;
    fresh.then_some(usage)
}

pub(super) fn context_summary(
    store: &SessionStore,
) -> Option<proteus_contracts::app_protocol::AppContextHistorySummary> {
    let reader = Reader::open(store.session_dir(), VIEW_CACHE)?;
    let header: Header = reader.one("header")?;
    if !matches(store, &header) {
        return None;
    }
    let (_, fresh) = reader.finish()?;
    fresh.then_some(header.context_summary)
}

fn matches(store: &SessionStore, header: &Header) -> bool {
    header.summary.session_id == store.session_id()
        && header.summary.session_dir == store.session_dir()
        && header.summary.message_count == header.conversation_len
        && store
            .workspace_path()
            .is_ok_and(|path| path == header.summary.workspace_path)
}

pub(super) fn page(
    store: &SessionStore,
    before: Option<usize>,
    expected: Option<&str>,
) -> Option<proteus_contracts::app_protocol::AppTranscriptPage> {
    page_inner(store, before, expected, false, 50)
}

pub(super) fn display_page(
    store: &SessionStore,
) -> Option<proteus_contracts::app_protocol::AppTranscriptPage> {
    page_inner(store, None, None, true, 50)
}

pub(super) fn transcript(
    store: &SessionStore,
    revision: &str,
) -> Option<Vec<AppTranscriptMessage>> {
    Some(page_inner(store, None, Some(revision), false, usize::MAX)?.messages)
}

fn page_inner(
    store: &SessionStore,
    before: Option<usize>,
    expected: Option<&str>,
    stale_display: bool,
    limit: usize,
) -> Option<proteus_contracts::app_protocol::AppTranscriptPage> {
    let reader = Reader::open(store.session_dir(), VIEW_CACHE)?;
    let header: Header = reader.one("header")?;
    if !matches(store, &header) {
        return None;
    }
    let end = before.unwrap_or(header.transcript_len);
    if end > header.transcript_len {
        return None;
    }
    let start = end.saturating_sub(limit);
    let messages = reader.page("transcript", start, end)?;
    let (source, fresh) = reader.finish()?;
    let revision = source.revision()?;
    // A client may keep reading an older completed display while a live turn
    // appends to the canonical journal. Never mix rows from different revisions.
    if expected.is_some_and(|value| value != revision)
        || (expected.is_none() && !fresh && !stale_display)
    {
        return None;
    }
    Some(proteus_contracts::app_protocol::AppTranscriptPage {
        revision,
        start,
        total: header.transcript_len,
        messages,
        base: header.base,
    })
}

//! Sections are decoded independently; text rows are fetched only when requested.
use super::*;
use crate::core::session_store::read_cache::{Reader, SourceFingerprint, Writer};

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

pub(super) fn save(
    store: &SessionStore,
    source: &SourceFingerprint,
    model: &SessionReadModel,
) -> bool {
    let save = || {
        let writer = Writer::new(store.session_dir(), VIEW_CACHE, source)?;
        let previous = Reader::open(store.session_dir(), VIEW_CACHE).filter(|reader| {
            reader
                .one::<Header>("header")
                .is_some_and(|header| matches(store, &header))
        });
        let mut base = previous
            .as_ref()
            .and_then(|reader| reader.revision())
            .map(
                |revision| proteus_contracts::app_protocol::AppTranscriptBase {
                    revision,
                    common_prefix: 0,
                },
            );
        writer.one("usage", &model.usage)?;
        for (index, message) in model.conversation.messages.iter().enumerate() {
            writer.row("conversation", index, message)?;
        }
        for (index, message) in model.transcript.iter().enumerate() {
            let hash = writer.row("transcript", index, message)?;
            if let (Some(base), Some(previous)) = (&mut base, &previous)
                && base.common_prefix == index
                && previous.row_hash("transcript", index).as_deref() == Some(hash.as_slice())
            {
                base.common_prefix += 1;
            }
        }
        for (index, observation) in model.conversation.model_context.iter().enumerate() {
            writer.row("context", index, observation)?;
        }
        writer.one(
            "header",
            &Header {
                summary: model.summary.clone(),
                revision: model.conversation.revision,
                conversation_len: model.conversation.messages.len(),
                transcript_len: model.transcript.len(),
                context_len: model.conversation.model_context.len(),
                context_summary: crate::core::summarize_context_history(
                    &model.conversation.messages,
                ),
                base,
            },
        )?;
        writer.finish()
    };
    save().is_some()
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

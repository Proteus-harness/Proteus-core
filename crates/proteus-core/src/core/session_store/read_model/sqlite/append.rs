//! A checkpoint and appended rows are published in one existing cache transaction.
use super::*;
use crate::core::{
    session_journal::{JournalReadCursor, JournalResumeState},
    session_store::read_cache::Updater,
};
use ring::digest::{SHA256, digest};

const PROJECTION_FORMAT: u32 = 1;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProjectionCheckpoint {
    format: u32,
    pub(super) cursor: JournalReadCursor,
    header_sha256: [u8; 32],
}

impl ProjectionCheckpoint {
    pub(super) fn new(header: &Header, cursor: &JournalReadCursor) -> Option<Self> {
        Some(Self {
            format: PROJECTION_FORMAT,
            cursor: cursor.clone(),
            header_sha256: header_hash(header)?,
        })
    }

    fn matches(&self, header: &Header) -> bool {
        self.format == PROJECTION_FORMAT && header_hash(header) == Some(self.header_sha256)
    }
}

pub(in crate::core::session_store::read_model) struct PreviousProjection {
    source: SourceFingerprint,
    header: Header,
    checkpoint: ProjectionCheckpoint,
}

impl PreviousProjection {
    pub(in crate::core::session_store::read_model) fn cursor(&self) -> &JournalReadCursor {
        &self.checkpoint.cursor
    }

    pub(in crate::core::session_store::read_model) fn conversation_len(&self) -> usize {
        self.header.conversation_len
    }
}

pub(in crate::core::session_store::read_model) fn checkpoint(
    store: &SessionStore,
) -> Option<PreviousProjection> {
    let reader = Reader::open(store.session_dir(), VIEW_CACHE)?;
    let header: Header = reader.one("header")?;
    let checkpoint: ProjectionCheckpoint = reader.one("projection")?;
    if !matches(store, &header) || !checkpoint.matches(&header) {
        return None;
    }
    let (source, _) = reader.finish()?;
    Some(PreviousProjection {
        source,
        header,
        checkpoint,
    })
}

pub(in crate::core::session_store::read_model) fn append(
    store: &SessionStore,
    source: &SourceFingerprint,
    previous: &PreviousProjection,
    cursor: &JournalReadCursor,
    state: &JournalResumeState,
    summary: &AppSessionSummary,
    suffix: &SessionTranscriptAppend,
) -> Option<()> {
    let writer = Updater::new(store.session_dir(), VIEW_CACHE, source)?;
    let header: Header = writer.previous("header")?;
    let checkpoint: ProjectionCheckpoint = writer.previous("projection")?;
    // Another full rebuild or append may have published while the renderer
    // ran. Never attach its suffix to a different cache generation.
    if !writer.source_matches(&previous.source)
        || checkpoint != previous.checkpoint
        || !checkpoint.matches(&header)
        || header_hash(&header) != header_hash(&previous.header)
        || !matches(store, &header)
        || state.history.len().checked_sub(header.conversation_len)
            != Some(suffix.conversation_appended)
    {
        return None;
    }
    super::tool_state::ensure_index(&writer, header.transcript_len)?;
    for (index, message) in state.history[header.conversation_len..].iter().enumerate() {
        writer.row("conversation", header.conversation_len + index, message)?;
    }
    for (index, message) in suffix.transcript.iter().enumerate() {
        writer.row(
            "transcript",
            header.transcript_len.checked_add(index)?,
            message,
        )?;
    }
    for row in &suffix.transcript {
        if let Some(tool) = &row.tool
            && accepted(&tool.status)
        {
            writer.keyed(
                TOOL_STATE,
                &tool.name,
                &proteus_contracts::app_protocol::AppToolState::from(tool),
            )?;
        }
    }
    let context = state.model_context.snapshot();
    for (index, observation) in context.iter().enumerate() {
        writer.row("context", index, observation)?;
    }
    writer.truncate("context", context.len())?;
    writer.one("usage", &state.usage)?;
    let next = Header {
        summary: summary.clone(),
        revision: state.history_revision,
        conversation_len: state.history.len(),
        transcript_len: header.transcript_len.checked_add(suffix.transcript.len())?,
        context_len: context.len(),
        context_summary: crate::core::summarize_context_history(&state.history),
        base: Some(proteus_contracts::app_protocol::AppTranscriptBase {
            revision: previous.source.revision()?,
            common_prefix: header.transcript_len,
        }),
    };
    writer.one("header", &next)?;
    writer.one("tool-state-index", &writer.keyed_fingerprint(TOOL_STATE)?)?;
    writer.one("projection", &ProjectionCheckpoint::new(&next, cursor)?)?;
    writer.finish()
}

fn header_hash(header: &Header) -> Option<[u8; 32]> {
    digest(&SHA256, &serde_json::to_vec(header).ok()?)
        .as_ref()
        .try_into()
        .ok()
}

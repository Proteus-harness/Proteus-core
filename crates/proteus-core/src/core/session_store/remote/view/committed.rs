//! Publish the display projection from records acknowledged by the writer.
use super::*;
use crate::core::history_message_sha256;
use proteus_contracts::{
    app_protocol::AppTranscriptBase,
    contracts::{StorageResumeState, StorageSnapshot},
};

use super::super::snapshot::DisplayRecords;

pub(crate) struct CommittedDisplay {
    pub(in crate::core::session_store::remote) start: StorageSnapshot,
    pub(in crate::core::session_store::remote) end: StorageSnapshot,
    pub(in crate::core::session_store::remote) state: StorageResumeState,
    pub(in crate::core::session_store::remote) records: DisplayRecords,
}

impl RemoteSessionStore {
    pub(crate) fn publish_committed_view(
        &self,
        committed: CommittedDisplay,
        render_append: impl FnOnce(
            &mut DisplayRecords,
            &mut PrefixIdentities,
            &[[u8; 32]],
            usize,
        ) -> Result<Option<SessionTranscriptAppend>>,
    ) -> Option<SessionReadView> {
        let CommittedDisplay {
            start,
            end,
            state,
            mut records,
        } = committed;
        let previous = cache::header(self)?;
        let baseline_source = RemoteCacheSource { snapshot: start };
        let source = RemoteCacheSource { snapshot: end };
        if previous.source != baseline_source
            || source.snapshot.revision != state.conversation.revision
        {
            return None;
        }
        let mut prefix = cache::read_prefix(self, previous.prefix.as_deref()?)?;
        let count = previous.conversation.rows;
        let history = &state.conversation.messages;
        let baseline = std::mem::take(&mut prefix.history);
        // Independent display appends always preserve the canonical prefix; a
        // compaction, reset or different content requires a complete projection.
        if baseline.len() != count
            || history
                .get(..count)?
                .iter()
                .zip(&baseline)
                .any(|(message, expected)| {
                    history_message_sha256(message).ok().as_ref() != Some(expected)
                })
        {
            return None;
        }
        let added = render_append(&mut records, &mut prefix, &baseline, history.len()).ok()??;
        if history.len().checked_sub(count)? != added.conversation_appended {
            return None;
        }
        prefix.history = baseline;
        for message in &history[count..] {
            prefix.history.push(history_message_sha256(message).ok()?);
        }
        let mut next = previous;
        let base = AppTranscriptBase {
            revision: next.source.revision()?,
            common_prefix: next.transcript.rows,
        };
        next.source = source.clone();
        next.summary = cache::summary(self, history).ok()?;
        next.revision = state.conversation.revision.history_revision;
        next.usage = state.usage;
        next.context_summary = crate::core::summarize_context_history(history);
        next.conversation =
            cache::extend_known_section(self, &source, &next.conversation, history)?;
        next.transcript =
            cache::append_section(self, &source, &next.transcript, &added.transcript)?;
        next.context = cache::write_section(self, &source, &state.conversation.model_context)?;
        next.cursor = Some(super::super::snapshot::RemoteJournalCursor {
            snapshot: source.snapshot.clone(),
        });
        next.base = Some(base);
        next.prefix = cache::write_prefix(self, &source, &prefix);
        cache::update_tools(self, &source, &mut next.tools, &added.transcript)?;
        cache::publish(self, &next)?;
        let (published, view, fresh) = cache::load_view(self)?;
        (fresh && published == source).then_some(view)
    }
}

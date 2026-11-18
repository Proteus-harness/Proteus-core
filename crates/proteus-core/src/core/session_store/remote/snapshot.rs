//! Fixed-prefix typed cursors over the selected service, never filesystem paths.
use super::*;
use proteus_contracts::{
    contracts::{StorageConversation, StorageHistoryPoint, StorageSnapshot},
    storage::journal::{JournalKind, JournalRecord},
};
use serde::{Deserialize, Serialize};

mod display_records;
pub(super) use display_records::DisplayRecordWriter;
pub(crate) use display_records::{DisplayRecords, is_display_kind};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteJournalCursor {
    pub(crate) snapshot: StorageSnapshot,
}
impl RemoteJournalCursor {
    pub(crate) fn last_session_seq(&self) -> u64 {
        self.snapshot.revision.session_seq
    }
}

pub(crate) struct RemoteJournalSnapshot {
    store: RemoteSessionStore,
    snapshot: StorageSnapshot,
    display_records: Option<DisplayRecords>,
}
impl std::fmt::Debug for RemoteJournalSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JournalReadSnapshot")
            .field("snapshot", &self.snapshot)
            .finish()
    }
}
impl RemoteJournalSnapshot {
    pub(crate) fn open(store: RemoteSessionStore) -> Result<Self> {
        let snapshot = match store.request(Request::OpenSnapshot {
            session: store.descriptor.session.clone(),
        })? {
            Response::Snapshot { snapshot } => snapshot,
            _ => bail!("storage returned an unexpected snapshot response"),
        };
        ensure!(
            snapshot.session == store.descriptor.session,
            "storage snapshot belongs to another session"
        );
        Ok(Self {
            store,
            snapshot,
            display_records: None,
        })
    }
    pub(crate) fn descriptor(&self) -> &StorageSnapshot {
        &self.snapshot
    }
    pub(crate) fn cursor(&self) -> RemoteJournalCursor {
        RemoteJournalCursor {
            snapshot: self.snapshot.clone(),
        }
    }
    pub(crate) fn conversation(&self, point: StorageHistoryPoint) -> Result<StorageConversation> {
        match self.store.request(Request::ReadConversation {
            snapshot: self.snapshot.clone(),
            point,
        })? {
            Response::Conversation { conversation } => Ok(conversation),
            _ => bail!("storage returned an unexpected conversation response"),
        }
    }
    pub(crate) fn scan(&mut self, consume: impl FnMut(JournalRecord) -> Result<()>) -> Result<()> {
        self.scan_from(0, |_| true, consume)
    }
    pub(crate) fn scan_display(
        &mut self,
        consume: impl FnMut(JournalRecord) -> Result<()>,
    ) -> Result<()> {
        match &mut self.display_records {
            Some(records) => records.scan(consume),
            None => self.scan_from(0, is_display_kind, consume),
        }
    }
    fn scan_from(
        &mut self,
        mut after_seq: u64,
        select: impl Fn(JournalKind) -> bool,
        mut consume: impl FnMut(JournalRecord) -> Result<()>,
    ) -> Result<()> {
        loop {
            let response = self.store.request(Request::ReadRecords {
                snapshot: self.snapshot.clone(),
                after_seq,
                limit: 64,
            })?;
            let Response::Records { page } = response else {
                bail!("storage returned an unexpected record page response")
            };
            ensure!(
                page.snapshot == self.snapshot,
                "storage changed the fixed snapshot while paging"
            );
            for record in page.records {
                ensure!(
                    record.session_id == self.snapshot.session.session_id
                        && record.session_seq > after_seq
                        && record.session_seq <= self.snapshot.revision.session_seq,
                    "storage record escaped the requested snapshot"
                );
                after_seq = record.session_seq;
                if select(record.entry.kind()) {
                    consume(record)?;
                }
            }
            match page.next_after_seq {
                Some(next) => {
                    ensure!(
                        next == after_seq && after_seq < self.snapshot.revision.session_seq,
                        "storage record page made no progress"
                    );
                }
                None => {
                    ensure!(
                        after_seq == self.snapshot.revision.session_seq,
                        "storage returned an incomplete snapshot"
                    );
                    return Ok(());
                }
            }
        }
    }
    pub(crate) fn scan_selected_after(
        &mut self,
        previous: &RemoteJournalCursor,
        select: fn(JournalKind) -> bool,
        consume: impl FnMut(JournalRecord) -> Result<()>,
    ) -> Result<bool> {
        if previous.snapshot.session != self.snapshot.session
            || previous.snapshot.revision.session_seq > self.snapshot.revision.session_seq
        {
            return Ok(false);
        }
        // Verify the immutable old prefix by a bounded pass before presenting a
        // suffix as append-only. The provider may reject an expired old handle.
        let old = match self.store.request(Request::CompareSnapshots {
            snapshot: self.snapshot.clone(),
            prefix: previous.snapshot.clone(),
        }) {
            Ok(Response::SnapshotPrefix { matches }) => matches,
            _ => false,
        };
        if !old {
            return Ok(false);
        }
        self.scan_from(previous.snapshot.revision.session_seq, select, consume)?;
        Ok(true)
    }
}
impl Drop for RemoteJournalSnapshot {
    fn drop(&mut self) {
        let _ = self.store.request(Request::ReleaseSnapshot {
            snapshot: self.snapshot.clone(),
        });
    }
}

#[derive(Debug)]
pub(crate) struct RemoteJournalResumeState {
    pub history: Vec<CanonicalMessage>,
    pub history_revision: u64,
    pub interrupted_turns: Vec<proteus_contracts::contracts::WorkflowHistoryInterruption>,
    pub model_context: proteus_session_semantics::ModelContextState,
    pub usage: proteus_contracts::domain::SessionUsageSnapshot,
    pub last_history_turn: Option<TurnId>,
}
impl From<proteus_contracts::contracts::StorageResumeState> for RemoteJournalResumeState {
    fn from(state: proteus_contracts::contracts::StorageResumeState) -> Self {
        Self {
            history: state.conversation.messages,
            history_revision: state.conversation.revision.history_revision,
            interrupted_turns: state.interrupted_turns,
            model_context: proteus_session_semantics::ModelContextState::from_snapshot(
                state.conversation.model_context,
            ),
            usage: state.usage,
            last_history_turn: state.last_history_turn,
        }
    }
}

pub(crate) struct RemoteValidatedRead {
    source: RemoteJournalSnapshot,
    resume: proteus_contracts::contracts::StorageResumeState,
    prefix_history_fingerprints: Option<Vec<[u8; 32]>>,
}
impl RemoteValidatedRead {
    pub(crate) fn open(store: RemoteSessionStore) -> Result<Self> {
        Self::validate(store, None, None).map(|(read, _)| read)
    }
    pub(crate) fn open_for_display(store: RemoteSessionStore) -> Result<Self> {
        Self::validate(store, None, DisplayRecordWriter::new()).map(|(read, _)| read)
    }
    pub(crate) fn open_with_cursor(
        store: RemoteSessionStore,
        previous: Option<&RemoteJournalCursor>,
    ) -> Result<(Self, bool)> {
        Self::validate(store, previous, DisplayRecordWriter::new())
    }
    fn validate(
        store: RemoteSessionStore,
        previous: Option<&RemoteJournalCursor>,
        mut display: Option<DisplayRecordWriter>,
    ) -> Result<(Self, bool)> {
        let mut source = store.snapshot()?;
        let prefix_matches = match previous {
            Some(previous) => matches!(
                store.request(Request::CompareSnapshots {
                    snapshot: source.snapshot.clone(),
                    prefix: previous.snapshot.clone()
                }),
                Ok(Response::SnapshotPrefix { matches: true })
            ),
            None => false,
        };
        let prefix_seq = previous
            .filter(|_| prefix_matches)
            .map(|previous| previous.last_session_seq());
        let mut prefix_history_fingerprints = prefix_seq.filter(|seq| *seq == 0).map(|_| vec![]);
        let mut validation = proteus_session_semantics::JournalValidationState::default();
        let mut usage = proteus_session_semantics::UsageProjection::default();
        let mut context = proteus_session_semantics::ModelContextProjection::default();
        let mut next_seq = 1;
        source.scan(|record| {
            validation.apply_sequenced(store.session_id(), &record, &mut next_seq)?;
            usage.apply(&record);
            context.apply(&record);
            if let Some(display) = &mut display
                && is_display_kind(record.entry.kind())
            {
                display.push(&record);
            }
            if prefix_seq == Some(record.session_seq) {
                prefix_history_fingerprints = Some(
                    validation
                        .history()
                        .iter()
                        .map(proteus_session_semantics::history_message_sha256)
                        .collect::<Result<_>>()?,
                );
            }
            Ok(())
        })?;
        source.display_records = display.and_then(DisplayRecordWriter::finish);
        let resume = proteus_contracts::contracts::StorageResumeState {
            conversation: StorageConversation {
                revision: source.snapshot.revision,
                model_context: context.snapshot(),
                messages: validation.history().to_vec(),
            },
            usage: usage.snapshot(store.session_id()),
            last_history_turn: validation.last_history_turn(),
            interrupted_turns: validation.interrupted_turns(),
        };
        ensure!(
            validation.history_revision() == source.snapshot.revision.history_revision,
            "storage snapshot history revision mismatch"
        );
        Ok((
            Self {
                source,
                resume,
                prefix_history_fingerprints,
            },
            prefix_matches,
        ))
    }
    pub(crate) fn cursor(&self) -> RemoteJournalCursor {
        self.source.cursor()
    }
    pub(crate) fn history(&self) -> &[CanonicalMessage] {
        &self.resume.conversation.messages
    }
    pub(crate) fn history_revision(&self) -> u64 {
        self.resume.conversation.revision.history_revision
    }
    pub(crate) fn interrupted_turns(
        &self,
    ) -> Vec<proteus_contracts::contracts::WorkflowHistoryInterruption> {
        self.resume.interrupted_turns.clone()
    }
    pub(crate) fn last_history_turn(&self) -> Option<TurnId> {
        self.resume.last_history_turn
    }
    pub(crate) fn model_context(
        &self,
    ) -> Vec<proteus_contracts::contracts::ModelContextObservation> {
        self.resume.conversation.model_context.clone()
    }
    pub(crate) fn usage(&self) -> proteus_contracts::domain::SessionUsageSnapshot {
        self.resume.usage.clone()
    }
    pub(crate) fn take_prefix_history_fingerprints(&mut self) -> Option<Vec<[u8; 32]>> {
        self.prefix_history_fingerprints.take()
    }
    pub(crate) fn into_parts(self) -> (RemoteJournalSnapshot, RemoteJournalResumeState) {
        (self.source, self.resume.into())
    }
    pub(crate) fn into_resume_state(self) -> RemoteJournalResumeState {
        self.resume.into()
    }
}

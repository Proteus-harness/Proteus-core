//! The view is updated inline with event delivery, before a producer can move
//! on. A subscription snapshot and its sequence are sampled under that same lock.
use super::{AppEventPublisher, AppServerEvent};
use crate::app_server::read_model::CompletedTranscript;
use crate::app_server::{AppTranscriptMessage, TurnProgress};
use crate::{
    contracts::EventSink,
    core::AgentRuntime,
    domain::{EventEnvelope, SessionId},
};
use anyhow::{Result, anyhow};
use proteus_contracts::app_protocol::{AppExecutionState, AppSessionSnapshot};
use std::sync::{Arc, OnceLock, Weak};

#[derive(Clone)]
pub(super) struct SequencedEvent {
    pub seq: u64,
    pub event: AppServerEvent,
}

pub(super) struct SessionView {
    session_id: SessionId,
    stream_id: String,
    pub seq: u64,
    runtime: Weak<AgentRuntime>,
    progress: TurnProgress,
    execution: AppExecutionState,
    // Completed turns are projected from the validated journal at attachment
    // and each turn boundary. Reconnect snapshots must not reread it under this lock.
    pub(super) history: Result<Arc<CompletedTranscript>, Arc<String>>,
}

struct SnapshotParts {
    session_id: SessionId,
    stream_id: String,
    seq: u64,
    root_thread_id: Option<crate::domain::ThreadId>,
    history: Arc<CompletedTranscript>,
    progress: Vec<AppTranscriptMessage>,
    execution: AppExecutionState,
}

impl SnapshotParts {
    fn into_snapshot(self) -> AppSessionSnapshot {
        let mut transcript = self.history.messages.clone();
        transcript.extend(self.progress);
        AppSessionSnapshot {
            session_id: self.session_id,
            stream_id: self.stream_id,
            seq: self.seq,
            root_thread_id: self.root_thread_id,
            transcript,
            history: self.history.cursor.clone(),
            execution: self.execution,
        }
    }
}

impl SessionView {
    pub(super) fn run_is_active(&self) -> bool {
        self.execution.active.is_some()
    }

    pub(super) fn session_dir(&self) -> Option<std::path::PathBuf> {
        self.runtime
            .upgrade()
            .and_then(|runtime| runtime.session_dir().map(std::path::Path::to_owned))
    }

    pub fn new(session_id: SessionId, stream_id: String) -> Self {
        Self {
            session_id,
            stream_id,
            seq: 0,
            runtime: Weak::new(),
            progress: TurnProgress::default(),
            execution: AppExecutionState::default(),
            history: Ok(Arc::new(CompletedTranscript::default())),
        }
    }

    pub fn apply(&mut self, event: &AppServerEvent) {
        self.seq = self.seq.checked_add(1).expect("session sequence");
        match event {
            AppServerEvent::Runtime { envelope } => self.progress.apply(envelope),
            AppServerEvent::UserMessageSubmitted { text, images } => {
                self.progress.submit_input(text.clone(), images.clone())
            }
            AppServerEvent::ExecutionUpdated { execution } => self.execution = execution.clone(),
            _ => {}
        }
    }

    fn snapshot_parts(&self) -> Result<SnapshotParts> {
        let history = self
            .history
            .as_ref()
            .map_err(|error| anyhow!("{error}"))?
            .clone();
        Ok(SnapshotParts {
            session_id: self.session_id,
            stream_id: self.stream_id.clone(),
            seq: self.seq,
            root_thread_id: self.progress.thread_id(),
            history,
            progress: self.progress.snapshot(),
            execution: self.execution.clone(),
        })
    }
}

impl AppEventPublisher {
    pub(in crate::app_server) fn completed_history(&self) -> Option<Arc<CompletedTranscript>> {
        self.0
            .view
            .lock()
            .expect("session view lock")
            .history
            .as_ref()
            .ok()
            .cloned()
    }

    pub(in crate::app_server) fn refresh_history(
        &self,
        expected: &Arc<CompletedTranscript>,
        history: CompletedTranscript,
    ) -> Result<()> {
        self.publish_refreshed_history(expected, history, false)
            .map(|_| ())
    }

    pub(super) fn refresh_idle_history(
        &self,
        expected: &Arc<CompletedTranscript>,
        history: CompletedTranscript,
    ) -> Result<bool> {
        self.publish_refreshed_history(expected, history, true)
    }

    fn publish_refreshed_history(
        &self,
        expected: &Arc<CompletedTranscript>,
        history: CompletedTranscript,
        idle_only: bool,
    ) -> Result<bool> {
        {
            let mut view = self.0.view.lock().expect("session view lock");
            if (idle_only && view.execution.active.is_some())
                || !view
                    .history
                    .as_ref()
                    .is_ok_and(|current| Arc::ptr_eq(current, expected))
            {
                return Ok(false);
            }
            view.history = Ok(Arc::new(history));
        }
        self.publish_snapshot()?;
        Ok(true)
    }

    pub(in crate::app_server) fn attach_runtime(
        &self,
        runtime: &Arc<AgentRuntime>,
        history: CompletedTranscript,
    ) {
        let mut view = self.0.view.lock().expect("session view lock");
        view.runtime = Arc::downgrade(runtime);
        view.history = Ok(Arc::new(history));
    }

    pub(in crate::app_server) fn session_snapshot(&self) -> Result<AppSessionSnapshot> {
        let parts = self
            .0
            .view
            .lock()
            .expect("session view lock")
            .snapshot_parts()?;
        Ok(parts.into_snapshot())
    }

    pub(in crate::app_server) fn publish_snapshot(&self) -> Result<()> {
        loop {
            let parts = self
                .0
                .view
                .lock()
                .expect("session view lock")
                .snapshot_parts()?;
            let previous_seq = parts.seq;
            let mut snapshot = parts.into_snapshot();
            snapshot.seq = previous_seq.checked_add(1).expect("session sequence");
            // Both transports own a DTO. Clone the completed transcript before
            // taking the event lock, then validate the sampled revision.
            let event = AppServerEvent::SessionSnapshot {
                snapshot: Box::new(snapshot),
            };
            let copy = event.clone();
            let mut view = self.0.view.lock().expect("session view lock");
            if view.seq != previous_seq {
                continue;
            }
            view.seq = previous_seq + 1;
            let _ = self.0.sequenced.send(SequencedEvent {
                seq: view.seq,
                event: copy,
            });
            let _ = self.0.events.send(event);
            return Ok(());
        }
    }

    pub(in crate::app_server) fn finish_progress(&self, history: Result<CompletedTranscript>) {
        let mut view = self.0.view.lock().expect("session view lock");
        view.seq = view.seq.checked_add(1).expect("session sequence");
        view.progress.finish_parent_turn();
        view.history = history
            .map(Arc::new)
            .map_err(|error| Arc::new(format!("{error:#}")));
    }

    pub(in crate::app_server) fn has_unstarted_input(&self) -> bool {
        self.0
            .view
            .lock()
            .expect("session view lock")
            .progress
            .has_unstarted_input()
    }

    pub(in crate::app_server) fn finish_run_progress(
        &self,
        history: Result<CompletedTranscript>,
        queued: Vec<crate::model_standard::CanonicalMessage>,
    ) {
        let mut view = self.0.view.lock().expect("session view lock");
        view.seq = view.seq.checked_add(1).expect("session sequence");
        if view.progress.has_unstarted_input() {
            // Admission echoed the input, but execution failed before TurnStarted
            // could consume it. Keep that non-durable row and the last validated
            // prefix; a corrupt/locked resume must still publish its terminal
            // execution error and a usable snapshot. The next submission replaces
            // this input, while explicit clear_history uses finish_progress.
            view.progress.retain_failed_queued_inputs(queued);
            return;
        }
        view.progress.finish_parent_turn();
        view.history = history
            .map(Arc::new)
            .map_err(|error| Arc::new(format!("{error:#}")));
    }
}

/// Bound after runtime assembly, before start_session/admission. No intermediate
/// broadcast queue can lose deltas from the authoritative in-memory view.
#[derive(Default)]
pub(in crate::app_server) struct RuntimeEventSink(pub OnceLock<AppEventPublisher>);

#[async_trait::async_trait]
impl EventSink for RuntimeEventSink {
    async fn append(&self, envelope: EventEnvelope) -> Result<()> {
        if let Some(events) = self.0.get() {
            let child_bound = matches!(
                envelope.event,
                crate::domain::Event::SubagentSessionBound { .. }
            );
            // Queued follow-ups share one app-server run, but commit separate
            // root turns. Preserve the completed prefix before TurnStarted
            // replaces live progress. The first turn already has its attached
            // prefix and must not trigger another journal projection.
            let runtime = if matches!(envelope.event, crate::domain::Event::TurnStarted { .. }) {
                let view = events.0.view.lock().expect("session view lock");
                match (&envelope.event, view.progress.turn_identity()) {
                    (
                        crate::domain::Event::TurnStarted { turn_id, .. },
                        Some((thread, previous)),
                    ) if thread == envelope.thread_id && previous != *turn_id => {
                        view.runtime.upgrade()
                    }
                    _ => None,
                }
            } else {
                None
            };
            // Root turns are serialized by the runtime reservation: the next
            // turn cannot advance while this sink awaits its prefix. Projection
            // runs outside the view lock; installation, reset and delivery below
            // share that lock so subscribers never sample an incomplete boundary.
            let history = match runtime {
                Some(runtime) => Some(crate::app_server::completed_transcript(runtime).await?),
                None => None,
            };
            let _ = events.send_with_history(
                AppServerEvent::Runtime {
                    envelope: Box::new(envelope),
                },
                history,
            );
            if child_bound {
                events.refresh_idle_child_binding();
            }
        }
        Ok(())
    }
}

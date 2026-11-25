//! Transport-neutral pending projection. State, revision and publication share
//! one lock; readers never assemble a snapshot from independently sampled maps.

use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, mpsc, watch};
use uuid::Uuid;

use super::{AppPendingRequests, AppQueuedUserMessage, AppServerEvent, StdioOutput};

mod binding_refresh;
mod session;
use crate::{core::QueuedMessagesSnapshot, domain::SessionId};
pub(super) use session::RuntimeEventSink;
use session::{SequencedEvent, SessionView};

#[derive(Clone)]
pub(super) struct AppEventPublisher(Arc<Inner>);

struct Inner {
    output: EventOutput,
    view: Mutex<SessionView>,
    pending: Mutex<Pending>,
    updates: watch::Sender<AppPendingRequests>,
    queue: Option<watch::Receiver<QueuedMessagesSnapshot>>,
    binding_refresh: Mutex<binding_refresh::State>,
}

enum EventOutput {
    Session {
        events: broadcast::Sender<AppServerEvent>,
        sequenced: broadcast::Sender<SequencedEvent>,
    },
    ProcessPeer(Mutex<Option<mpsc::WeakSender<StdioOutput>>>),
}

struct Pending {
    snapshot: AppPendingRequests,
    queue_revision: u64,
}

/// Both transports receive initial pending and session snapshots. Session
/// events already included in a snapshot are discarded; overflow triggers a
/// fresh baseline. Pending updates use an independent watch projection.
pub(crate) struct AppSubscription {
    publisher: AppEventPublisher,
    events: broadcast::Receiver<SequencedEvent>,
    pending: watch::Receiver<AppPendingRequests>,
    initial: bool,
    resync: bool,
    seq: u64,
}

impl AppSubscription {
    pub(crate) fn request_snapshot(&mut self) {
        self.resync = true;
    }

    pub(crate) async fn recv(&mut self) -> anyhow::Result<AppServerEvent> {
        if self.initial {
            self.initial = false;
            return Ok(self.pending_event());
        }
        loop {
            if self.resync {
                let snapshot = self.publisher.session_snapshot()?;
                self.seq = snapshot.seq;
                self.resync = false;
                return Ok(AppServerEvent::SessionSnapshot {
                    snapshot: Box::new(snapshot),
                });
            }
            tokio::select! {
                biased;
                changed = self.pending.changed() => {
                    changed?;
                    return Ok(self.pending_event());
                }
                event = self.events.recv() => match event {
                    Ok(event) if event.seq > self.seq || matches!(event.event, AppServerEvent::Shutdown) => {
                        self.seq = event.seq;
                        return Ok(event.event);
                    }
                    Ok(_) => {}, // Already included in the snapshot.
                    Err(broadcast::error::RecvError::Lagged(count)) => {
                        self.resync = true;
                        return Ok(AppServerEvent::EventStreamLagged { count });
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }

    fn pending_event(&mut self) -> AppServerEvent {
        AppServerEvent::PendingRequestsUpdated {
            snapshot: Box::new(self.pending.borrow_and_update().clone()),
        }
    }
}

impl AppEventPublisher {
    pub(super) fn new(
        capacity: usize,
        session_id: SessionId,
        queue: Option<watch::Receiver<QueuedMessagesSnapshot>>,
    ) -> Self {
        Self::with_output(
            session_id,
            queue,
            EventOutput::Session {
                events: broadcast::channel(capacity).0,
                sequenced: broadcast::channel(capacity).0,
            },
        )
    }

    pub(super) fn for_process_peer(
        session_id: SessionId,
        output: &mpsc::Sender<StdioOutput>,
    ) -> Self {
        Self::with_output(
            session_id,
            None,
            EventOutput::ProcessPeer(Mutex::new(Some(output.downgrade()))),
        )
    }

    fn with_output(
        session_id: SessionId,
        queue: Option<watch::Receiver<QueuedMessagesSnapshot>>,
        output: EventOutput,
    ) -> Self {
        let snapshot = AppPendingRequests::new(session_id, Uuid::new_v4().to_string());
        let publisher = Self(Arc::new(Inner {
            output,
            view: Mutex::new(SessionView::new(session_id, snapshot.stream_id.clone())),
            pending: Mutex::new(Pending {
                snapshot: snapshot.clone(),
                queue_revision: 0,
            }),
            updates: watch::channel(snapshot).0,
            queue: queue.clone(),
            binding_refresh: Mutex::new(binding_refresh::State::default()),
        }));
        if let Some(mut queue) = queue {
            let events = publisher.clone();
            tokio::spawn(async move {
                while queue.changed().await.is_ok() {
                    // Read the latest value under the projection lock, never
                    // replay a possibly superseded notification payload.
                    events.pending_snapshot();
                }
            });
        }
        publisher
    }

    pub(super) fn subscribe(&self) -> broadcast::Receiver<AppServerEvent> {
        let EventOutput::Session { events, .. } = &self.0.output else {
            unreachable!("process peers forward to their stdio writer");
        };
        events.subscribe()
    }

    pub(super) fn subscribe_session(&self) -> AppSubscription {
        let EventOutput::Session { sequenced, .. } = &self.0.output else {
            unreachable!("process peers have no display snapshot");
        };
        AppSubscription {
            publisher: self.clone(),
            events: sequenced.subscribe(),
            pending: self.subscribe_pending(),
            initial: true,
            resync: true,
            seq: 0,
        }
    }

    pub(super) fn subscribe_pending(&self) -> watch::Receiver<AppPendingRequests> {
        self.pending_snapshot();
        self.0.updates.subscribe()
    }

    pub(super) fn pending_snapshot(&self) -> AppPendingRequests {
        let mut pending = self.0.pending.lock().expect("pending projection lock");
        if self.sync_queue(&mut pending) {
            self.publish_pending(&mut pending);
        }
        pending.snapshot.clone()
    }

    pub(super) fn send(
        &self,
        event: AppServerEvent,
    ) -> Result<usize, broadcast::error::SendError<AppServerEvent>> {
        if !self.projects_transcript() {
            return Ok(0);
        }
        let idle = matches!(&event, AppServerEvent::ExecutionUpdated { execution } if execution.active.is_none());
        let result = self.send_with_history(event, None);
        if idle {
            self.drain_child_bindings();
        }
        result
    }

    pub(super) fn projects_transcript(&self) -> bool {
        matches!(self.0.output, EventOutput::Session { .. })
    }

    pub(super) fn detach_process_peer(&self) {
        if let EventOutput::ProcessPeer(output) = &self.0.output {
            output.lock().expect("process peer output lock").take();
        }
    }

    /// Peer events and command replies share one ordered writer queue. Interactive
    /// requests wait for queue space and cannot be overwritten by runtime events.
    pub(super) async fn send_required(&self, event: AppServerEvent) -> anyhow::Result<()> {
        match &self.0.output {
            EventOutput::ProcessPeer(output) => {
                let writer = {
                    let route = output.lock().expect("process peer output lock");
                    let Some(writer) = route.as_ref() else {
                        // A retired session can still have a background producer.
                        return Ok(());
                    };
                    writer.upgrade()
                };
                let writer = writer
                    .ok_or_else(|| anyhow::anyhow!("process peer stdout writer is closed"))?;
                let permit = writer
                    .reserve()
                    .await
                    .map_err(|_| anyhow::anyhow!("process peer stdout writer is closed"))?;
                let route = output.lock().expect("process peer output lock");
                if route.is_some() {
                    // Retiring a session fences writers that were waiting for space.
                    permit.send(StdioOutput::Event {
                        event: Box::new(event),
                    });
                }
                Ok(())
            }
            EventOutput::Session { .. } => {
                let _ = self.send(event);
                Ok(())
            }
        }
    }

    fn send_with_history(
        &self,
        event: AppServerEvent,
        history: Option<super::read_model::CompletedTranscript>,
    ) -> Result<usize, broadcast::error::SendError<AppServerEvent>> {
        let EventOutput::Session { events, sequenced } = &self.0.output else {
            return Ok(0);
        };
        let mut view = self.0.view.lock().expect("session view lock");
        if let Some(history) = history {
            view.history = Ok(Arc::new(history));
        }
        view.apply(&event);
        let mut pending = self.0.pending.lock().expect("pending projection lock");
        let queue_changed = self.sync_queue(&mut pending);
        let changed = match &event {
            AppServerEvent::ApprovalRequested { request } => {
                let items = &mut pending.snapshot.approvals;
                items.retain(|item| item.approval_id != request.approval_id);
                items.push((**request).clone());
                items.sort_by(|a, b| a.seq.cmp(&b.seq).then(a.approval_id.cmp(&b.approval_id)));
                true
            }
            AppServerEvent::ApprovalResolved { approval_id, .. } => {
                let items = &mut pending.snapshot.approvals;
                let before = items.len();
                items.retain(|item| &item.approval_id != approval_id);
                items.len() != before
            }
            AppServerEvent::UserInputRequested { request } => {
                let items = &mut pending.snapshot.user_inputs;
                items.retain(|item| item.request_id != request.request_id);
                items.push((**request).clone());
                items.sort_by(|a, b| a.seq.cmp(&b.seq).then(a.request_id.cmp(&b.request_id)));
                true
            }
            AppServerEvent::UserInputResolved { request_id } => {
                let items = &mut pending.snapshot.user_inputs;
                let before = items.len();
                items.retain(|item| &item.request_id != request_id);
                items.len() != before
            }
            _ => false,
        };
        let _ = sequenced.send(SequencedEvent {
            seq: view.seq,
            event: event.clone(),
        });
        let result = events.send(event);
        if changed || queue_changed {
            self.publish_pending(&mut pending);
        }
        result
    }

    fn sync_queue(&self, pending: &mut Pending) -> bool {
        let Some(queue) = &self.0.queue else {
            return false;
        };
        let queue = queue.borrow();
        if queue.revision == pending.queue_revision {
            return false;
        }
        pending.queue_revision = queue.revision;
        pending.snapshot.queued_user_messages = queue
            .messages
            .iter()
            .map(|(id, text, image_count)| AppQueuedUserMessage {
                message_id: *id,
                text: text.clone(),
                image_count: *image_count,
            })
            .collect();
        true
    }

    fn publish_pending(&self, pending: &mut Pending) {
        pending.snapshot.seq = pending
            .snapshot
            .seq
            .checked_add(1)
            .expect("pending sequence");
        // Complete snapshots need only one retained value, regardless of
        // subscriber speed. Intermediate revisions can safely be coalesced.
        self.0.updates.send_replace(pending.snapshot.clone());
    }
}

#[cfg(test)]
pub(super) fn test_event_channel(
    capacity: usize,
) -> (AppEventPublisher, broadcast::Receiver<AppServerEvent>) {
    let events = AppEventPublisher::new(capacity, crate::domain::new_session_id(), None);
    let receiver = events.subscribe();
    (events, receiver)
}

#[cfg(test)]
mod tests;

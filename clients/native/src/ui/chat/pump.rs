//! The live session stream: connection state and server events folded into
//! the chat.

use std::path::PathBuf;

use gpui_kit::*;
use proteus_contracts::app_protocol::AppServerEvent;
use proteus_contracts::domain::Event as RuntimeEvent;

use super::context_usage::ContextUsage;
use super::{Chat, ChatEvent};
use crate::api::Api;
use crate::connection::{self, PumpItem};
use crate::transcript::{Change, Role};

/// Streams the session into the chat until the chat is dropped; dropping
/// the task closes the channel, which stops the stream thread.
pub(super) fn start(api: Api, session_dir: PathBuf, cx: &mut Context<Chat>) -> Task<()> {
    let events = connection::spawn_pump(api, session_dir);
    cx.spawn(async move |this, cx| {
        while let Ok(first) = events.recv().await {
            let mut batch = vec![first];
            while let Ok(item) = events.try_recv() {
                batch.push(item);
            }
            let applied = this.update(cx, |this, cx| {
                let mut shown = false;
                for item in batch {
                    shown |= this.apply(item, cx);
                }
                // Copies of other sessions' activity and runtime events
                // without a trace in the chat leave it as drawn.
                if shown {
                    cx.notify();
                }
            });
            if applied.is_err() {
                break;
            }
        }
    })
}

impl Chat {
    /// Folds one stream item in; true when the chat looks different.
    fn apply(&mut self, item: PumpItem, cx: &mut Context<Self>) -> bool {
        let session = &mut self.session;
        match item {
            PumpItem::Connected => {
                session.cursor.begin_connection();
                session.pending_cursor.begin_connection();
                session.connected = true;
                session.link_error = None;
                true
            }
            PumpItem::Disconnected(error) => {
                session.connected = false;
                session.link_error = Some(error.into());
                true
            }
            PumpItem::Event(event) => self.apply_event(*event, cx),
        }
    }

    fn apply_event(&mut self, event: AppServerEvent, cx: &mut Context<Self>) -> bool {
        if let AppServerEvent::SessionActivityUpdated {
            session_dir,
            activity,
        } = event
        {
            cx.emit(ChatEvent::Activity {
                session_dir,
                activity,
            });
            return false;
        }
        let session = &mut self.session;
        if !session.cursor.accept(&event) {
            return false;
        }
        let mut shown = true;
        let change = match event {
            AppServerEvent::SessionSnapshot { snapshot } => {
                let snapshot = *snapshot;
                session.execution = snapshot.execution;
                session.root_thread = snapshot.root_thread_id;
                session.transcript.reset(snapshot.transcript)
            }
            AppServerEvent::ExecutionUpdated { execution } => {
                let settled = session.running() && execution.active.is_none();
                session.execution = execution;
                if settled {
                    self.load_context(cx);
                    cx.emit(ChatEvent::RunSettled);
                }
                Change::None
            }
            AppServerEvent::Runtime { envelope } => {
                if let RuntimeEvent::TokenUsageUpdated { usage, .. } = &envelope.event
                    && session
                        .root_thread
                        .is_none_or(|root| root == envelope.thread_id)
                {
                    session.context = ContextUsage::from_event(usage);
                } else {
                    shown = false;
                }
                session
                    .transcript
                    .apply_runtime(envelope.thread_id, &envelope.event)
            }
            AppServerEvent::UserMessageSubmitted { text, images } => {
                session.transcript.push_user(None, &text, images)
            }
            AppServerEvent::PendingRequestsUpdated { snapshot } => {
                if session.pending_cursor.accept_stream(
                    &snapshot.session_id.to_string(),
                    &snapshot.stream_id,
                    snapshot.seq,
                ) {
                    let shown = session.pending.as_ref();
                    for key in super::requests::request_keys(&snapshot) {
                        if !shown.is_some_and(|shown| {
                            super::requests::request_keys(shown).any(|old| old == key)
                        }) {
                            self.fresh_requests.mark(key);
                        }
                    }
                    session.pending = Some(*snapshot);
                }
                if let Some(pending) = &session.pending {
                    self.user_input.retain(pending);
                }
                Change::None
            }
            AppServerEvent::ModulesReloaded { .. }
            | AppServerEvent::ProfileReloadStatus { error: None } => {
                self.load_config(cx);
                Change::None
            }
            AppServerEvent::Error { message } => {
                self.show_notice(message, cx);
                Change::None
            }
            AppServerEvent::ProfileReloadStatus { error: Some(error) } => {
                self.show_notice(format!("Не удалось перезагрузить профиль: {error}"), cx);
                Change::None
            }
            AppServerEvent::Shutdown => {
                cx.emit(ChatEvent::ServerStopped);
                Change::None
            }
            _ => Change::None,
        };
        shown |= !matches!(change, Change::None);
        self.apply_change(change, cx);
        shown
    }

    fn apply_change(&mut self, change: Change, cx: &mut Context<Self>) {
        if matches!(change, Change::Spliced { .. }) {
            let entries = self.session.transcript.entries();
            self.replies
                .retain(&entries.iter().map(|entry| entry.key).collect());
            let replies = entries
                .iter()
                .filter(|entry| entry.role == Role::Assistant)
                .map(|entry| (entry.key, &entry.text));
            self.replies.prepare(replies, cx);
        }
        if matches!(change, Change::Pushed)
            && let Some(entry) = self.session.transcript.entries().last()
        {
            self.fresh_rows.mark(entry.key);
        }
        self.scroller.update(cx, |scroller, cx| match change {
            Change::None => {}
            Change::Pushed => {
                scroller.append(1, cx);
            }
            Change::Updated(row) => {
                scroller.remeasure_items(row..row + 1, cx);
            }
            Change::Spliced {
                start,
                removed,
                inserted,
            } => {
                scroller.splice(start..start + removed, inserted, cx);
            }
        });
        let changed = match change {
            Change::Pushed => self.session.transcript.entries().len().checked_sub(1),
            Change::Updated(row) => Some(row),
            Change::None | Change::Spliced { .. } => None,
        };
        if let Some(row) = changed {
            self.remeasure_chain(row, cx);
        }
    }
}

//! One stdio turn on a leased process child: event forwarding, interactive
//! request bridging, usage tracking and cancel/timeout protocol.

use std::{sync::Mutex, time::Duration};

use anyhow::{Context as _, Result, bail};
use proteus_contracts::app_protocol::{AppServerEvent, StdioOutput, StdioRequest};
use tokio::time::{Instant, timeout_at};

use super::super::mailbox::ChildMailbox;
use super::{
    child::ChildProcess,
    messaging::{PeerMessageDelivery, PeerMessageResponse},
};
use crate::{
    contracts::{
        AgentLifecycleStatus, AgentWorkflowContext, ApprovalRequest, ExecutionAttribution,
        RequestOrigin, UserInputResponse,
    },
    domain::{AgentOutput, Event, EventContext, ThreadId},
};

/// Терминальный исход turn-а ребёнка (fatal-ошибки идут через `Err`).
pub(super) enum TurnEnd {
    Completed(String),
    Interrupted(AgentLifecycleStatus),
}

#[derive(Default)]
pub(super) struct TurnTracker {
    pub(super) iterations: u32,
    /// Текст завершённых model-ответов и хвост текущего стрима — единственный
    /// источник partial summary при cancel/timeout.
    last_text: Option<String>,
    stream_buffer: String,
    cancel_sent: bool,
    /// Messages confirmed by the peer app-server during this logical
    /// generation. The initial task prompt is not counted.
    pub(super) delivered_messages: u64,
    /// SessionStarted allocates identity before the first write. Publish the
    /// link only after accepted turn evidence proves its history is readable.
    pub(super) session_bound: bool,
}

impl TurnTracker {
    pub(super) fn observe(&mut self, event: &Event) {
        match event {
            Event::ModelRequestPrepared { .. } => {
                self.iterations = self.iterations.saturating_add(1);
                self.stream_buffer.clear();
            }
            Event::AssistantTextDelta { text, .. } => {
                self.stream_buffer.push_str(text);
            }
            Event::ModelResponseReceived { .. } => {
                if !self.stream_buffer.trim().is_empty() {
                    self.last_text = Some(std::mem::take(&mut self.stream_buffer));
                } else {
                    self.stream_buffer.clear();
                }
            }
            _ => {}
        }
    }

    pub(super) fn partial_text(&self) -> String {
        if !self.stream_buffer.trim().is_empty() {
            self.stream_buffer.clone()
        } else {
            self.last_text.clone().unwrap_or_default()
        }
    }
}

/// Пере-эмиссия событий ребёнка в родительский event stream под
/// `child_thread_id` и форвардинг интерактивных запросов.
pub(super) struct ChildEventForwarder<'a> {
    pub(super) ctx: &'a AgentWorkflowContext,
    pub(super) child_thread_id: ThreadId,
    pub(super) role: String,
    pub(super) source_call_id: Option<String>,
    pub(super) source_attribution: ExecutionAttribution,
}

impl ChildEventForwarder<'_> {
    async fn bind_session(&self, child: &ChildProcess, tracker: &mut TurnTracker) -> Result<()> {
        if !tracker.session_bound {
            if let Some(session) = child.session() {
                if let Some(source_call_id) = &self.source_call_id {
                    self.ctx
                        .tool_recorder
                        .subagent_session_bound(
                            self.source_attribution,
                            &crate::domain::SubagentSessionBinding {
                                source_call_id: source_call_id.clone(),
                                child_thread_id: self.child_thread_id,
                                session_id: session.session_id,
                                session_dir: session.session_dir.clone(),
                            },
                        )
                        .await?;
                }
                self.ctx
                    .emit(Event::SubagentSessionBound {
                        child_thread_id: self.child_thread_id,
                        session_id: session.session_id,
                        session_dir: session.session_dir.clone(),
                    })
                    .await?;
                tracker.session_bound = true;
            }
        }
        Ok(())
    }

    fn event_context(&self) -> EventContext {
        EventContext::new(
            self.ctx.session_id,
            self.child_thread_id,
            Some(self.ctx.turn_id),
        )
    }

    fn origin(&self) -> RequestOrigin {
        RequestOrigin::for_turn(
            self.ctx.execution.scope.execution_id,
            self.child_thread_id,
            self.ctx.turn_id,
        )
        .with_label(self.role.clone())
    }

    async fn forward_runtime_event(&self, event: Event) {
        if !should_forward_child_event(&event) {
            return;
        }
        let _ = self.ctx.events.emit(self.event_context(), event).await;
    }

    async fn forward_approval(
        &self,
        request: proteus_contracts::app_protocol::AppApprovalRequest,
    ) -> Option<StdioRequest> {
        let approval_id = request.approval_id.clone();
        let forwarded = ApprovalRequest::new(
            request.call.clone(),
            request.cwd.clone(),
            request.reason.clone(),
            request.tool_spec.clone(),
        )
        .with_origin(self.origin());
        let response = tokio::select! {
            response = self.ctx.execution.approval.request_approval(forwarded) => response,
            _ = self.ctx.execution.scope.cancellation.cancelled() => return None,
        };
        let (approved, note, cache) = match response {
            Ok(response) => (response.approved, response.note, response.cache),
            Err(error) => (
                false,
                Some(format!("parent approval transport failed: {error:#}")),
                Default::default(),
            ),
        };
        Some(StdioRequest::Approval {
            id: None,
            approval_id,
            approved,
            note,
            cache,
        })
    }

    async fn forward_user_input(
        &self,
        request: crate::contracts::UserInputRequest,
    ) -> Option<StdioRequest> {
        let request_id = request.request_id.clone();
        let forwarded = request.with_origin(self.origin());
        let response = tokio::select! {
            response = self.ctx.user_input.request_user_input(forwarded) => response,
            _ = self.ctx.execution.scope.cancellation.cancelled() => return None,
        };
        let response = response.unwrap_or_else(|_| UserInputResponse::empty());
        Some(StdioRequest::UserInput {
            id: None,
            request_id,
            response,
        })
    }
}

pub(super) fn should_forward_child_event(event: &Event) -> bool {
    matches!(
        event,
        Event::ToolCallRequested { .. }
            | Event::ToolCallResolved { .. }
            | Event::ApprovalRequested { .. }
            | Event::ApprovalResolved { .. }
            | Event::ToolFinished { .. }
            | Event::SubagentStarted { .. }
            | Event::SubagentSessionBound { .. }
            | Event::SubagentFinished { .. }
            | Event::Error { .. }
    )
}

pub(super) async fn drive_turn(
    child: &mut ChildProcess,
    forwarder: &ChildEventForwarder<'_>,
    send_id: &str,
    tracker: &mut TurnTracker,
    cancel_grace: Duration,
    mailbox: &ChildMailbox,
    published_active_send_id: &Mutex<String>,
) -> Result<TurnEnd> {
    let mut active_send_id = send_id.to_owned();
    let mut active_started_by_message = false;
    let mut active_terminal_seen = false;
    let mut message_delivery = PeerMessageDelivery::default();
    let mut pending_terminal = None;
    let mut pending_from_message_turn = false;

    loop {
        if forwarder.ctx.execution.scope.cancellation.is_cancelled() && !tracker.cancel_sent {
            return finish_cancelled(
                child,
                forwarder,
                &active_send_id,
                tracker,
                cancel_grace,
                message_delivery.is_settled(),
            )
            .await;
        }

        let delivery_guard = mailbox.lock_delivery().await;
        if forwarder.ctx.execution.scope.cancellation.is_cancelled() && !tracker.cancel_sent {
            drop(delivery_guard);
            return finish_cancelled(
                child,
                forwarder,
                &active_send_id,
                tracker,
                cancel_grace,
                message_delivery.is_settled(),
            )
            .await;
        }
        if active_terminal_seen && message_delivery.is_settled() && pending_terminal.is_some() {
            let messages = mailbox.drain_or_close()?;
            if messages.is_empty() {
                return Ok(pending_terminal
                    .take()
                    .expect("terminal checked before mailbox close"));
            }
            active_send_id = message_delivery.start_continuation(child, messages).await?;
            *published_active_send_id
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = active_send_id.clone();
            active_started_by_message = true;
            active_terminal_seen = false;
            pending_terminal = None;
            pending_from_message_turn = false;
        } else {
            let messages = mailbox.drain()?;
            if !messages.is_empty() {
                message_delivery.queue(child, messages).await?;
            }
        }
        drop(delivery_guard);

        let output = tokio::select! {
            output = child.next_output() => output,
            _ = forwarder.ctx.execution.scope.cancellation.cancelled() => {
                return finish_cancelled(
                    child,
                    forwarder,
                    &active_send_id,
                    tracker,
                    cancel_grace,
                    message_delivery.is_settled(),
                )
                .await;
            }
            _ = mailbox.notified() => continue,
        };
        let Some(output) = output? else {
            bail!("subagent child process exited unexpectedly");
        };

        match handle_output(
            child,
            forwarder,
            &active_send_id,
            tracker,
            &mut message_delivery,
            output,
        )
        .await?
        {
            OutputVerdict::Continue => {}
            OutputVerdict::Finished(TurnEnd::Completed(text)) => {
                active_terminal_seen = true;
                if active_started_by_message {
                    tracker.delivered_messages = tracker.delivered_messages.saturating_add(1);
                }
                if !pending_from_message_turn {
                    pending_terminal = Some(TurnEnd::Completed(text));
                }
            }
            OutputVerdict::MessageTurnCompleted(text) => {
                tracker.delivered_messages = tracker.delivered_messages.saturating_add(1);
                pending_terminal = Some(TurnEnd::Completed(text));
                pending_from_message_turn = true;
            }
            OutputVerdict::Finished(end) => return Ok(end),
            OutputVerdict::CancelRequested => {
                return finish_cancelled(
                    child,
                    forwarder,
                    &active_send_id,
                    tracker,
                    cancel_grace,
                    message_delivery.is_settled(),
                )
                .await;
            }
        }
    }
}

async fn finish_cancelled(
    child: &mut ChildProcess,
    forwarder: &ChildEventForwarder<'_>,
    send_id: &str,
    tracker: &mut TurnTracker,
    cancel_grace: Duration,
    messages_settled: bool,
) -> Result<TurnEnd> {
    // A pending Send may have started a new turn after the original terminal
    // response. Targeted cancellation of the old request cannot prove idle.
    if !messages_settled {
        child.kill().await;
        return Ok(TurnEnd::Interrupted(AgentLifecycleStatus::Cancelled));
    }
    finish_interrupted_with(
        child,
        forwarder,
        send_id,
        tracker,
        cancel_grace,
        AgentLifecycleStatus::Cancelled,
    )
    .await
}

async fn finish_interrupted_with(
    child: &mut ChildProcess,
    forwarder: &ChildEventForwarder<'_>,
    send_id: &str,
    tracker: &mut TurnTracker,
    cancel_grace: Duration,
    status: AgentLifecycleStatus,
) -> Result<TurnEnd> {
    let clean = cancel_child_turn(child, forwarder, send_id, tracker, cancel_grace).await;
    if !clean {
        child.kill().await;
    }
    Ok(TurnEnd::Interrupted(status))
}

enum OutputVerdict {
    Continue,
    Finished(TurnEnd),
    MessageTurnCompleted(String),
    CancelRequested,
}

async fn handle_output(
    child: &mut ChildProcess,
    forwarder: &ChildEventForwarder<'_>,
    send_id: &str,
    tracker: &mut TurnTracker,
    message_delivery: &mut PeerMessageDelivery,
    output: StdioOutput,
) -> Result<OutputVerdict> {
    match output {
        StdioOutput::Event { event } => match *event {
            AppServerEvent::Runtime { envelope } => {
                if matches!(envelope.event, Event::TurnStarted { .. }) {
                    forwarder.bind_session(child, tracker).await?;
                }
                tracker.observe(&envelope.event);
                forwarder.forward_runtime_event(envelope.event).await;
                Ok(OutputVerdict::Continue)
            }
            AppServerEvent::SessionSnapshot { snapshot } => {
                if snapshot.root_thread_id.is_some() {
                    forwarder.bind_session(child, tracker).await?;
                }
                Ok(OutputVerdict::Continue)
            }
            AppServerEvent::ApprovalRequested { request } => {
                let health = child.output_health();
                let reply = tokio::select! {
                    biased;
                    error = health.stopped() => return Err(error),
                    reply = forwarder.forward_approval(*request) => reply,
                };
                match reply {
                    Some(reply) => {
                        child.send(&reply).await?;
                        Ok(OutputVerdict::Continue)
                    }
                    None => Ok(OutputVerdict::CancelRequested),
                }
            }
            AppServerEvent::UserInputRequested { request } => {
                let health = child.output_health();
                let reply = tokio::select! {
                    biased;
                    error = health.stopped() => return Err(error),
                    reply = forwarder.forward_user_input(*request) => reply,
                };
                match reply {
                    Some(reply) => {
                        child.send(&reply).await?;
                        Ok(OutputVerdict::Continue)
                    }
                    None => Ok(OutputVerdict::CancelRequested),
                }
            }
            AppServerEvent::Shutdown => bail!("subagent child app-server shut down mid-turn"),
            _ => Ok(OutputVerdict::Continue),
        },
        StdioOutput::Response {
            id: Some(id),
            ok,
            output,
            error,
        } => {
            if let Some(response) =
                message_delivery.confirm(&id, ok, output.as_ref(), error.as_deref())?
            {
                return Ok(match response {
                    PeerMessageResponse::Queued => {
                        tracker.delivered_messages = tracker.delivered_messages.saturating_add(1);
                        OutputVerdict::Continue
                    }
                    PeerMessageResponse::TurnCompleted(output) => {
                        OutputVerdict::MessageTurnCompleted(output.text)
                    }
                });
            }
            if id != send_id {
                return Ok(OutputVerdict::Continue);
            }
            if ok {
                let output = output.context("subagent child returned an empty Send output")?;
                let text = serde_json::from_value::<AgentOutput>(output)
                    .context("subagent child returned an invalid Send output")?
                    .text;
                forwarder.bind_session(child, tracker).await?;
                return Ok(OutputVerdict::Finished(TurnEnd::Completed(text)));
            }
            if tracker.cancel_sent {
                return Ok(OutputVerdict::Finished(TurnEnd::Interrupted(
                    AgentLifecycleStatus::Cancelled,
                )));
            }
            bail!(
                "subagent child turn failed: {}",
                error.unwrap_or_else(|| "unknown error".to_owned())
            );
        }
        StdioOutput::Response { .. } => Ok(OutputVerdict::Continue),
    }
}

pub(super) async fn cancel_child_turn(
    child: &mut ChildProcess,
    forwarder: &ChildEventForwarder<'_>,
    send_id: &str,
    tracker: &mut TurnTracker,
    cancel_grace: Duration,
) -> bool {
    let deadline = Instant::now() + cancel_grace;
    child.set_write_budget(Some(deadline), None);
    if !tracker.cancel_sent {
        tracker.cancel_sent = true;
        let cancel = StdioRequest::Cancel {
            id: None,
            target_id: send_id.to_owned(),
        };
        if child.send(&cancel).await.is_err() {
            return false;
        }
    }

    loop {
        let output = match timeout_at(deadline, child.next_output()).await {
            Ok(Ok(Some(output))) => output,
            Ok(Ok(None) | Err(_)) | Err(_) => return false,
        };
        match output {
            StdioOutput::Response { id, .. } if id.as_deref() == Some(send_id) => return true,
            StdioOutput::Event { event } => {
                if let AppServerEvent::Runtime { envelope } = *event {
                    tracker.observe(&envelope.event);
                    forwarder.forward_runtime_event(envelope.event).await;
                }
            }
            _ => {}
        }
    }
}

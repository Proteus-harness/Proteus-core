//! Execution-owned tool events and terminal publication for each actual call.
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::{
    contracts::{
        EventEmitter, ToolTerminalController, ToolTerminalDescriptor, ToolTerminalHost,
        ToolTerminalRef,
    },
    domain::{CallId, Event, EventContext, ToolCall, ToolResult},
};

pub(crate) type ToolTerminalFactory =
    Arc<dyn Fn(&CallId) -> Arc<dyn ToolTerminalHost> + Send + Sync>;

pub(crate) fn observer_for_turn(
    events: Arc<EventEmitter>,
    context: EventContext,
) -> Arc<dyn super::ToolExecutionObserver> {
    Arc::new(EventToolExecutionObserver { events, context })
}

struct EventToolExecutionObserver {
    events: Arc<EventEmitter>,
    context: EventContext,
}

impl EventToolExecutionObserver {
    async fn emit(&self, event: Event) -> Result<()> {
        self.events.emit(self.context.clone(), event).await
    }
}

#[async_trait]
impl super::ToolExecutionObserver for EventToolExecutionObserver {
    async fn tool_call_requested(&self, call: &ToolCall) -> Result<()> {
        self.emit(Event::ToolCallRequested { call: call.clone() })
            .await
    }

    async fn tool_call_resolved(&self, call: &ToolCall) -> Result<()> {
        self.emit(Event::ToolCallResolved { call: call.clone() })
            .await
    }

    async fn approval_requested(&self, call: &ToolCall, reason: &str) -> Result<()> {
        self.emit(Event::ApprovalRequested {
            call_id: call.id.clone(),
            reason: reason.to_owned(),
        })
        .await
    }

    async fn approval_resolved(&self, call: &ToolCall, approved: bool) -> Result<()> {
        self.emit(Event::ApprovalResolved {
            call_id: call.id.clone(),
            approved,
        })
        .await
    }

    async fn tool_finished(&self, result: &ToolResult) -> Result<()> {
        self.emit(Event::ToolFinished {
            result: result.clone(),
        })
        .await
    }
}

pub(crate) fn for_turn(
    registry: Arc<dyn ToolTerminalHost>,
    events: Arc<EventEmitter>,
    context: EventContext,
) -> ToolTerminalFactory {
    Arc::new(move |call_id| {
        Arc::new(EventTerminalHost {
            registry: registry.clone(),
            events: events.clone(),
            context: context.clone(),
            call_id: call_id.clone(),
        })
    })
}

struct EventTerminalHost {
    registry: Arc<dyn ToolTerminalHost>,
    events: Arc<EventEmitter>,
    context: EventContext,
    call_id: CallId,
}

#[async_trait]
impl ToolTerminalHost for EventTerminalHost {
    async fn publish(
        &self,
        descriptor: ToolTerminalDescriptor,
        controller: Arc<dyn ToolTerminalController>,
    ) -> Result<ToolTerminalRef> {
        let terminal = self.registry.publish(descriptor, controller).await?;
        self.events
            .emit(
                self.context.clone(),
                Event::ToolTerminalBound {
                    call_id: self.call_id.clone(),
                    terminal: terminal.clone(),
                },
            )
            .await?;
        Ok(terminal)
    }
}

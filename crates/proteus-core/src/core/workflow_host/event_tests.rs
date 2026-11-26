use super::*;
use crate::{
    contracts::{
        AgentWorkflowContext, CancellationToken, EventEmitter, EventSink, ExecutionContext,
        ExecutionScope, ToolRegistry,
    },
    domain::{EventEnvelope, new_session_id, new_thread_id, new_turn_id},
};
use async_trait::async_trait;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::Notify, time::timeout};

#[derive(Default)]
struct Sink {
    seen: Mutex<Vec<EventEnvelope>>,
    entered: Notify,
    release: Notify,
    fail: bool,
}

#[async_trait]
impl EventSink for Sink {
    async fn append(&self, envelope: EventEnvelope) -> Result<()> {
        self.seen.lock().unwrap().push(envelope);
        self.entered.notify_one();
        self.release.notified().await;
        anyhow::ensure!(!self.fail, "fixture event sink failed");
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_event_keeps_attribution_sink_error_and_cancellation() {
    for (sink_fails, cancel_while_pending) in [(false, false), (true, false), (false, true)] {
        let sink = Arc::new(Sink {
            fail: sink_fails,
            ..Default::default()
        });
        let cancellation = CancellationToken::new();
        let execution = ExecutionContext::new(
            ExecutionScope::fresh(cancellation.clone()),
            0,
            None,
            ToolRegistry::default(),
            Arc::new(crate::stubs::DenyAllPolicy),
            Arc::new(crate::core::HeadlessApprovalTransport),
        );
        let (session, thread, turn) = (new_session_id(), new_thread_id(), new_turn_id());
        let ctx = AgentWorkflowContext::new(
            execution,
            session,
            thread,
            turn,
            None,
            Default::default(),
            1000,
            Arc::new(EventEmitter::new(sink.clone())),
            Arc::new(crate::stubs::EmptyContextBuilder),
            Arc::new(crate::core::HeadlessUserInputTransport),
            crate::test_model::no_context_preparation(),
            None,
        );
        let host = Arc::new(WorkflowHostRuntime::new(ctx.into(), ".".into()));
        let event = Event::TaskReceived {
            task: AgentTask::new("inert event", ".".into()),
        };
        let pending = {
            let host = Arc::clone(&host);
            let event = event.clone();
            // Mirror the async dispatcher: the owner may be moved into a Send future.
            tokio::spawn(async move { host.emit_event(event).await })
        };
        let limit = Duration::from_secs(5);
        timeout(limit, sink.entered.notified()).await.unwrap();
        assert!(!pending.is_finished());
        if cancel_while_pending {
            cancellation.cancel();
        } else {
            sink.release.notify_one();
        }
        let result = timeout(limit, pending).await.unwrap().unwrap();
        match (sink_fails, cancel_while_pending) {
            (true, false) => assert_eq!(result.unwrap_err().to_string(), "fixture event sink failed"),
            (false, true) => assert_eq!(result.unwrap_err().to_string(), "turn canceled by client"),
            _ => result.unwrap(),
        }

        // A cancelled new event never reaches the sink, including after an
        // earlier successful or failing append to the same emitter.
        cancellation.cancel();
        assert_eq!(
            timeout(limit, host.emit_event(event.clone()))
                .await
                .unwrap()
                .unwrap_err()
                .to_string(),
            "turn canceled by client"
        );
        let envelopes = sink.seen.lock().unwrap();
        assert_eq!(envelopes.len(), 1);
        let recorded = &envelopes[0];
        assert_eq!(recorded.seq, 1);
        assert_eq!(
            (recorded.session_id, recorded.thread_id, recorded.turn_id),
            (session, thread, Some(turn))
        );
        assert_eq!(recorded.event, event);
    }
}

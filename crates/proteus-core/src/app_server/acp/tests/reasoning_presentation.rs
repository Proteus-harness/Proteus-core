//! Bound presentation feeds the actual ACP thought projection; no providers.
use super::*;
use crate::{
    contracts::{
        CancellationToken, EventEmitter, EventSink, ExecutionScope, Model, ModelEventStream,
        NoopExecutionRecorder,
    },
    core::{BoundModel, ModelExecutionBinding, ModelService},
    domain::ModelRef,
    model_standard::{
        CanonicalMessage, CanonicalModelRequest, CanonicalModelResponse, ContentPart, FinishReason,
        MessageRole, ModelCapabilities, ModelStreamEvent,
    },
};
use async_trait::async_trait;
use std::{borrow::Cow, sync::Mutex as StdMutex};

struct Scripted(StdMutex<Option<Vec<ModelStreamEvent>>>);
#[async_trait]
impl Model for Scripted {
    fn id(&self) -> Cow<'static, str> {
        "reasoning-fixture".into()
    }
    fn capabilities(&self, _: &ModelRef) -> anyhow::Result<ModelCapabilities> {
        Ok(ModelCapabilities::empty())
    }
    async fn stream(&self, _: CanonicalModelRequest) -> anyhow::Result<ModelEventStream> {
        let events = self.0.lock().unwrap().take().unwrap();
        Ok(Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok),
        )))
    }
}
#[derive(Default)]
struct Captured(StdMutex<Vec<EventEnvelope>>);
#[async_trait]
impl EventSink for Captured {
    async fn append(&self, event: EventEnvelope) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

#[tokio::test]
async fn anonymous_progress_is_not_republished_as_identified_completed_thoughts() {
    for (anonymous, identified, expected) in [
        (true, false, vec!["readable"]),
        (false, false, vec!["readable"]),
        (true, true, vec!["summary", "readable"]),
    ] {
        let message = CanonicalMessage::new(
            MessageRole::Assistant,
            vec![
                ContentPart::ReasoningSummary {
                    text: "readable".into(),
                },
                ContentPart::Text {
                    text: "answer".into(),
                },
            ],
        );
        let response = CanonicalModelResponse::new(message.clone(), vec![], FinishReason::Stop);
        let mut events = vec![];
        if anonymous {
            events.push(ModelStreamEvent::ReasoningSummaryDelta {
                text: if identified { "summary" } else { "readable" }.into(),
            });
        }
        if identified {
            events.push(ModelStreamEvent::ReasoningDelta {
                message_id: message.id,
                text: "readable".into(),
            });
        }
        events.push(ModelStreamEvent::MessageCompleted {
            message: message.clone(),
        });
        events.push(ModelStreamEvent::Response { response });
        let session = new_session_id();
        let thread = new_thread_id();
        let turn = crate::domain::new_turn_id();
        let captured = Arc::new(Captured::default());
        let binding = ModelExecutionBinding::for_turn(
            ExecutionScope::fresh(CancellationToken::new()),
            Arc::new(EventEmitter::new(captured.clone())),
            session,
            thread,
            turn,
            Arc::new(NoopExecutionRecorder),
        );
        let model = BoundModel::new(
            Arc::new(ModelService::new(Arc::new(Scripted(StdMutex::new(Some(
                events,
            )))))),
            binding,
            0,
        );
        model
            .complete(CanonicalModelRequest::new(
                ModelRef::new("fixture", "fixture"),
                vec![],
            ))
            .await
            .unwrap();
        let mut projection = projection::Projection::new(session);
        projection
            .event(EventEnvelope::new(
                EventContext::new(session, thread, Some(turn)),
                0,
                Event::TurnStarted {
                    session_id: session,
                    thread_id: thread,
                    turn_id: turn,
                },
            ))
            .unwrap();
        let mut thoughts = vec![];
        let mut completions = 0;
        for envelope in captured.0.lock().unwrap().iter().cloned() {
            if matches!(
                envelope.event,
                Event::AssistantReasoningMessageCompleted { .. }
            ) {
                completions += 1;
            }
            for update in projection.event(envelope).unwrap() {
                let value = serde_json::to_value(update).unwrap();
                if value["sessionUpdate"] == "agent_thought_chunk" {
                    thoughts.push(value["content"]["text"].as_str().unwrap().to_owned());
                }
            }
        }
        assert_eq!(
            thoughts, expected,
            "anonymous={anonymous}, identified={identified}"
        );
        assert_eq!(completions, usize::from(!anonymous || identified));
        let cold = crate::app_server::transcript::transcript_messages(&[message]);
        assert_eq!(cold.iter().filter(|row| row.role == "reasoning").count(), 1);
        assert_eq!(cold[0].text, "readable");
        assert_eq!(cold[1].text, "answer");
    }
}

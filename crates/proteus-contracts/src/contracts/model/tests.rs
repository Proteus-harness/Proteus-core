use super::*;
use std::sync::Mutex;

use crate::model_standard::{
    CanonicalMessage, FinishReason, MessageRole, ModelFailure, ModelFailureKind, TokenUsage,
};
use futures_util::stream;

struct StreamOnlyModel {
    next_stream: Mutex<Option<Result<ModelEventStream>>>,
}

impl StreamOnlyModel {
    fn events(events: Vec<Result<ModelStreamEvent>>) -> Self {
        Self {
            next_stream: Mutex::new(Some(Ok(Box::pin(stream::iter(events))))),
        }
    }

    fn start_failure(failure: ModelFailure) -> Self {
        Self {
            next_stream: Mutex::new(Some(Err(anyhow::Error::new(failure)))),
        }
    }
}

#[async_trait]
impl Model for StreamOnlyModel {
    fn id(&self) -> Cow<'static, str> {
        "stream-only".into()
    }

    fn capabilities(&self, _model: &ModelRef) -> Result<ModelCapabilities> {
        Ok(ModelCapabilities::empty())
    }

    async fn stream(&self, _request: CanonicalModelRequest) -> Result<ModelEventStream> {
        self.next_stream
            .lock()
            .unwrap()
            .take()
            .expect("one invocation")
    }
}

fn request() -> CanonicalModelRequest {
    CanonicalModelRequest::new(ModelRef::new("test", "model"), Vec::new())
}

fn failure(kind: ModelFailureKind) -> ModelFailure {
    ModelFailure::new(kind, "typed terminal diagnosis").with_completed_messages(vec![
        CanonicalMessage::text(MessageRole::Assistant, "completed item"),
    ])
}

#[tokio::test]
async fn inherited_complete_preserves_the_entire_canonical_failure() {
    for kind in [
        ModelFailureKind::ContextWindowExceeded,
        ModelFailureKind::StreamDisconnected,
        ModelFailureKind::Retryable {
            retry_delay_ms: Some(17),
        },
        ModelFailureKind::Interrupted,
        ModelFailureKind::SessionBudgetExceeded,
        ModelFailureKind::Other,
    ] {
        let expected = failure(kind);
        let model = StreamOnlyModel::events(vec![
            Ok(ModelStreamEvent::MessageCompleted {
                message: expected.completed_messages[0].clone(),
            }),
            Ok(ModelStreamEvent::Error {
                failure: expected.clone(),
            }),
        ]);

        let error = model.complete(request()).await.unwrap_err();

        assert_eq!(error.downcast_ref::<ModelFailure>(), Some(&expected));
        assert_eq!(ModelFailure::from_error(&error), expected);
    }
}

#[tokio::test]
async fn inherited_complete_returns_the_terminal_response_unchanged() {
    let expected = CanonicalModelResponse::new(
        CanonicalMessage::text(MessageRole::Assistant, "final answer"),
        Vec::new(),
        FinishReason::Stop,
    )
    .with_usage(TokenUsage::new(12, 3));
    let model = StreamOnlyModel::events(vec![
        Ok(ModelStreamEvent::TextDelta {
            message_id: expected.messages[0].id,
            phase: None,
            text: "final ".into(),
        }),
        Ok(ModelStreamEvent::Response {
            response: expected.clone(),
        }),
    ]);

    assert_eq!(model.complete(request()).await.unwrap(), expected);
}

#[tokio::test]
async fn inherited_complete_keeps_start_and_stream_item_errors_typed() {
    let expected = failure(ModelFailureKind::Interrupted);
    let start = StreamOnlyModel::start_failure(expected.clone());
    let item = StreamOnlyModel::events(vec![Err(
        anyhow::Error::new(expected.clone()).context("stream item context"),
    )]);
    for model in [start, item] {
        let error = model.complete(request()).await.unwrap_err();
        assert_eq!(ModelFailure::from_error(&error), expected);
    }
}

#[tokio::test]
async fn inherited_complete_retains_the_true_eof_error() {
    for events in [
        Vec::new(),
        vec![Ok(ModelStreamEvent::MessageCompleted {
            message: CanonicalMessage::text(MessageRole::Assistant, "not a terminal response"),
        })],
    ] {
        let error = StreamOnlyModel::events(events)
            .complete(request())
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "model stream ended without a complete response"
        );
        assert!(error.downcast_ref::<ModelFailure>().is_none());
    }
}

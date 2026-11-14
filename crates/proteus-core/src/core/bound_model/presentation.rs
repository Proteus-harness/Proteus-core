use super::{Event, ModelEventStream, ModelExecutionBinding, ModelStreamEvent};
use futures_util::StreamExt;

pub(super) fn present_stream(
    mut stream: ModelEventStream,
    binding: ModelExecutionBinding,
    suppress_stream_deltas: bool,
    deadline: Option<tokio::time::Instant>,
    cancellation: crate::contracts::CancellationToken,
) -> ModelEventStream {
    Box::pin(async_stream::try_stream! {
        let mut text_offsets = std::collections::HashMap::new();
        let mut reasoning_offsets = std::collections::HashMap::new();
        let mut completed = std::collections::HashMap::new();
        let mut anonymous_summary_published = false;

        while let Some(event) = stream.next().await {
            let event = event?;
            match &event {
                ModelStreamEvent::Response { response } => {
                    if !suppress_stream_deltas {
                        for message in &response.messages {
                            if !binding
                                .emit_message(message, &mut completed, anonymous_summary_published, deadline, &cancellation)
                                .await
                            {
                                break;
                            }
                        }
                    }
                }
                ModelStreamEvent::TextDelta {
                    message_id,
                    phase,
                    text,
                } if !suppress_stream_deltas => {
                    let cursor = text_offsets.entry(*message_id).or_insert(0);
                    let offset = *cursor;
                    *cursor += text.len();
                    binding
                        .emit_delta_before_deadline(
                            Event::AssistantTextDelta {
                                offset,
                                message_id: *message_id,
                                phase: *phase,
                                text: text.clone(),
                            },
                            deadline,
                            &cancellation,
                        )
                        .await;
                }
                ModelStreamEvent::ReasoningDelta { message_id, text } if !suppress_stream_deltas => {
                    let cursor = reasoning_offsets.entry(*message_id).or_insert(0);
                    let offset = *cursor;
                    *cursor += text.len();
                    completed.entry(*message_id).or_insert_with(CompletedMessage::default).reasoning_streamed = true;
                    binding
                        .emit_delta_before_deadline(
                            Event::AssistantReasoningTextDelta {
                                message_id: *message_id,
                                offset,
                                text: text.clone(),
                            },
                            deadline,
                            &cancellation,
                        )
                        .await;
                }
                ModelStreamEvent::MessageCompleted { message } if !suppress_stream_deltas => {
                    binding
                        .emit_message(message, &mut completed, anonymous_summary_published, deadline, &cancellation)
                        .await;
                }
                ModelStreamEvent::ToolCallDelta {
                    call_id,
                    args_delta,
                    ..
                } if !suppress_stream_deltas => {
                    binding
                        .emit_delta_before_deadline(
                            Event::AssistantToolArgsDelta {
                                call_id: call_id.clone(),
                                args_delta: args_delta.clone(),
                            },
                            deadline,
                            &cancellation,
                        )
                        .await;
                }
                ModelStreamEvent::ReasoningSummaryDelta { text } if !suppress_stream_deltas => {
                    anonymous_summary_published |= binding
                        .emit_delta_before_deadline(
                            Event::AssistantReasoningDelta { text: text.clone() },
                            deadline,
                            &cancellation,
                        )
                        .await;
                }
                _ => {}
            }
            yield event;
        }
    })
}

#[derive(Default)]
struct CompletedMessage {
    text: Option<(Option<crate::model_standard::MessagePhase>, String)>,
    reasoning: Option<String>,
    reasoning_streamed: bool,
}

impl ModelExecutionBinding {
    async fn emit_message(
        &self,
        message: &crate::model_standard::CanonicalMessage,
        completed: &mut std::collections::HashMap<crate::domain::MessageId, CompletedMessage>,
        anonymous_summary_published: bool,
        deadline: Option<tokio::time::Instant>,
        cancellation: &crate::contracts::CancellationToken,
    ) -> bool {
        let state = completed.entry(message.id).or_default();
        let reasoning = message.reasoning_text();
        // Anonymous summaries have already been published as progress, but cannot
        // be associated honestly with any canonical item. Identified reasoning
        // still owns its completion; terminal-only reasoning has no prior progress.
        if (!anonymous_summary_published || state.reasoning_streamed)
            && (!reasoning.is_empty() || state.reasoning_streamed || state.reasoning.is_some())
            && state.reasoning.as_ref() != Some(&reasoning)
        {
            state.reasoning = Some(reasoning.clone());
            if !self
                .emit_delta_before_deadline(
                    Event::AssistantReasoningMessageCompleted {
                        message_id: message.id,
                        text: reasoning,
                    },
                    deadline,
                    cancellation,
                )
                .await
            {
                return false;
            }
        }
        let text = message.display_text();
        let identity = (message.phase, text);
        if !identity.1.is_empty() && state.text.as_ref() != Some(&identity) {
            state.text = Some(identity.clone());
            return self
                .emit_delta_before_deadline(
                    Event::AssistantMessageCompleted {
                        message_id: message.id,
                        phase: identity.0,
                        text: identity.1,
                    },
                    deadline,
                    cancellation,
                )
                .await;
        }
        true
    }
}

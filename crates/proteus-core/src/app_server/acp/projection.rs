//! Append-only ACP projection of one canonical root turn.
use crate::domain::{Event, EventEnvelope, MessageId, SessionId as RuntimeSessionId, ThreadId};
use agent_client_protocol::{Result, schema::v1::*};
use std::collections::{HashMap, HashSet};

pub(super) struct Projection {
    session: RuntimeSessionId,
    root: Option<ThreadId>,
    texts: HashMap<MessageId, String>,
    thoughts: HashMap<MessageId, String>,
    last_message: Option<MessageId>,
    tools: HashSet<String>,
}

impl Projection {
    pub(super) fn new(session: RuntimeSessionId) -> Self {
        Self {
            session,
            root: None,
            texts: HashMap::new(),
            thoughts: HashMap::new(),
            last_message: None,
            tools: HashSet::new(),
        }
    }

    pub(super) fn event(&mut self, envelope: EventEnvelope) -> Result<Vec<SessionUpdate>> {
        if envelope.session_id != self.session {
            return Ok(vec![]);
        }
        if matches!(envelope.event, Event::TurnStarted { .. }) && self.root.is_none() {
            self.root = Some(envelope.thread_id);
        }
        if self.root != Some(envelope.thread_id) {
            return Ok(vec![]);
        }
        let update = match envelope.event {
            Event::AssistantTextDelta {
                message_id,
                offset,
                text,
                ..
            } => {
                let current = self.texts.entry(message_id).or_default();
                if offset > current.len() || !current.is_char_boundary(offset) {
                    return Err(super::internal("assistant stream has a gap"));
                }
                let overlap = current.len() - offset;
                let shared = overlap.min(text.len());
                if !current.is_char_boundary(offset + shared)
                    || !text.is_char_boundary(shared)
                    || current[offset..offset + shared] != text[..shared]
                {
                    return Err(super::internal(
                        "assistant stream changed already published text",
                    ));
                }
                let suffix = append_delta(current, text, shared);
                self.message_chunk(message_id, suffix)
            }
            Event::AssistantMessageCompleted {
                message_id, text, ..
            } => {
                let current = self.texts.entry(message_id).or_default();
                let Some(suffix) = text.strip_prefix(current.as_str()) else {
                    return Err(super::internal(
                        "completed assistant message changed published text",
                    ));
                };
                let suffix = suffix.to_owned();
                *current = text;
                self.message_chunk(message_id, suffix)
            }
            Event::AssistantReasoningTextDelta {
                message_id,
                offset,
                text,
            } => self.thought_chunk(message_id, Some(offset), text)?,
            Event::AssistantReasoningMessageCompleted { message_id, text } => {
                self.thought_chunk(message_id, None, text)?
            }
            Event::AssistantReasoningDelta { text } => Some(SessionUpdate::AgentThoughtChunk(
                ContentChunk::new(ContentBlock::Text(TextContent::new(text))),
            )),
            Event::ToolCallRequested { call } => {
                self.tools.insert(call.id.to_string());
                Some(SessionUpdate::ToolCall(
                    ToolCall::new(call.id.to_string(), call.name)
                        .status(ToolCallStatus::Pending)
                        .raw_input(call.args),
                ))
            }
            Event::ApprovalResolved { call_id, approved } => {
                Some(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    call_id.to_string(),
                    ToolCallUpdateFields::new().status(if approved {
                        ToolCallStatus::InProgress
                    } else {
                        ToolCallStatus::Failed
                    }),
                )))
            }
            Event::ToolFinished { result } => {
                self.tools.remove(&result.call_id.to_string());
                let plan = super::tool_updates::plan(&result);
                let text = result
                    .error
                    .clone()
                    .unwrap_or_else(|| result.output.clone());
                let update = SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    result.call_id.to_string(),
                    ToolCallUpdateFields::new()
                        .status(if result.ok {
                            ToolCallStatus::Completed
                        } else {
                            ToolCallStatus::Failed
                        })
                        .content(vec![ToolCallContent::Content(Content::new(
                            ContentBlock::Text(TextContent::new(text)),
                        ))])
                        .raw_output(serde_json::to_value(result).map_err(super::internal)?),
                ));
                return Ok(std::iter::once(update).chain(plan).collect());
            }
            _ => None,
        };
        Ok(update.into_iter().collect())
    }

    pub(super) fn fallback_output(&self, text: String) -> Option<SessionUpdate> {
        if self.texts.values().all(String::is_empty) {
            message(text)
        } else {
            None
        }
    }

    fn thought_chunk(
        &mut self,
        message_id: MessageId,
        offset: Option<usize>,
        text: String,
    ) -> Result<Option<SessionUpdate>> {
        let current = self.thoughts.entry(message_id).or_default();
        let suffix = if let Some(offset) = offset {
            if offset > current.len() || !current.is_char_boundary(offset) {
                return Err(super::internal("reasoning stream has a gap"));
            }
            let overlap = (current.len() - offset).min(text.len());
            if !text.is_char_boundary(overlap)
                || !current.is_char_boundary(offset + overlap)
                || current[offset..offset + overlap] != text[..overlap]
            {
                return Err(super::internal("reasoning stream changed published text"));
            }
            append_delta(current, text, overlap)
        } else {
            let Some(suffix) = text.strip_prefix(current.as_str()) else {
                return Err(super::internal(
                    "completed reasoning changed published text",
                ));
            };
            let suffix = suffix.to_owned();
            *current = text;
            suffix
        };
        Ok((!suffix.is_empty()).then(|| {
            SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(suffix),
            )))
        }))
    }

    fn message_chunk(&mut self, id: MessageId, text: String) -> Option<SessionUpdate> {
        if text.is_empty() {
            return None;
        }
        let separated = self.last_message.is_some_and(|previous| previous != id);
        self.last_message = Some(id);
        message(if separated {
            format!("\n\n{text}")
        } else {
            text
        })
    }

    pub(super) fn settle_tools(&mut self) -> Vec<SessionUpdate> {
        self.tools
            .drain()
            .map(|id| {
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    id,
                    ToolCallUpdateFields::new()
                        .status(ToolCallStatus::Failed)
                        .content(vec![ToolCallContent::Content(Content::new(
                            ContentBlock::Text(TextContent::new(
                                "Turn ended before this tool completed",
                            )),
                        ))]),
                ))
            })
            .collect()
    }
}

/// Append the verified new bytes and return just those bytes for the ACP update.
fn append_delta(current: &mut String, text: String, shared: usize) -> String {
    let suffix = &text[shared..];
    current.push_str(suffix);
    // Do not keep an oversized input buffer alive for a small notification.
    if shared == 0 && text.len() == text.capacity() {
        text
    } else {
        suffix.to_owned()
    }
}

pub(super) fn message(text: String) -> Option<SessionUpdate> {
    (!text.is_empty()).then(|| {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    })
}

#[cfg(test)]
#[test]
fn verified_delta_reuses_only_a_tight_whole_chunk() {
    let mut current = String::new();
    let whole = Box::<str>::from("é").into_string();
    let ptr = whole.as_ptr();
    let first = append_delta(&mut current, whole, 0);
    assert_eq!(first.as_ptr(), ptr);

    let mut spare = String::with_capacity(1024);
    spare.push('界');
    let next = append_delta(&mut current, spare, 0);
    assert_eq!(next, "界");
    assert!(next.capacity() < 1024);

    let tail = append_delta(&mut current, "界!".to_owned(), "界".len());
    assert_eq!(tail, "!");
    assert_eq!(current, "é界!");
}

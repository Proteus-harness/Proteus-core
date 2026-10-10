//! Client-side transcript: the server snapshot plus live runtime events folded
//! the same way the server folds turn progress, so that the next snapshot
//! replaces it without visible jumps.

use std::collections::HashMap;
use std::sync::Arc;

use proteus_contracts::app_protocol::{
    AppTranscriptMessage, AppTranscriptSubagent, AppTranscriptTool,
};
use proteus_contracts::domain::{Event, MessageId, ThreadId, ToolResult};
use serde_json::Value;

const MAX_SUBAGENT_TOOLS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// Stable client identity, reused across snapshots for the same message.
    pub key: u64,
    pub message_id: Option<MessageId>,
    pub role: Role,
    pub text: Arc<str>,
    pub images: usize,
    pub tool: Option<AppTranscriptTool>,
    pub subagent: Option<AppTranscriptSubagent>,
    pub streaming: bool,
}

/// What a fold step changed, for list virtualization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    Spliced {
        start: usize,
        removed: usize,
        inserted: usize,
    },
    Pushed,
    Updated(usize),
}

#[derive(Default)]
pub struct Transcript {
    entries: Vec<Entry>,
    turn_thread: Option<ThreadId>,
    next_key: u64,
}

impl Transcript {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Replace the transcript with a server snapshot. Rows that did not change
    /// keep their key and text allocation, so the change is reported as a
    /// splice from the first differing row instead of a full reset.
    pub fn reset(&mut self, messages: Vec<AppTranscriptMessage>) -> Change {
        let old = std::mem::take(&mut self.entries);
        let mut previous: HashMap<Identity, (u64, Arc<str>)> = old
            .iter()
            .filter_map(|entry| Some((identity_of(entry)?, (entry.key, entry.text.clone()))))
            .collect();
        let mut entries = Vec::with_capacity(messages.len());
        for (index, message) in messages.into_iter().enumerate() {
            let mut entry = self.entry_from(message);
            let reused = match identity_of(&entry) {
                Some(id) => previous.remove(&id),
                // Rows without identity are matched by position and content.
                None => old
                    .get(index)
                    .filter(|before| {
                        identity_of(before).is_none()
                            && before.role == entry.role
                            && before.text == entry.text
                    })
                    .map(|before| (before.key, before.text.clone())),
            };
            if let Some((key, text)) = reused {
                entry.key = key;
                if text == entry.text {
                    entry.text = text;
                }
            }
            entries.push(entry);
        }
        let start = old
            .iter()
            .zip(&entries)
            .position(|(before, after)| !same_row(before, after))
            .unwrap_or(old.len().min(entries.len()));
        let change = Change::Spliced {
            start,
            removed: old.len() - start,
            inserted: entries.len() - start,
        };
        self.entries = entries;
        match change {
            Change::Spliced {
                removed: 0,
                inserted: 0,
                ..
            } => Change::None,
            change => change,
        }
    }

    pub fn push_user(
        &mut self,
        message_id: Option<MessageId>,
        text: &str,
        images: usize,
    ) -> Change {
        self.stop_streaming();
        let key = self.allocate_key();
        self.entries.push(Entry {
            key,
            message_id,
            role: Role::User,
            text: text.into(),
            images,
            tool: None,
            subagent: None,
            streaming: false,
        });
        Change::Pushed
    }

    pub fn apply_runtime(&mut self, thread_id: ThreadId, event: &Event) -> Change {
        match event {
            Event::TurnStarted { .. } => {
                self.turn_thread = Some(thread_id);
                Change::None
            }
            Event::AssistantTextDelta {
                offset,
                message_id,
                text,
                ..
            } if self.is_turn_thread(thread_id) => {
                self.assistant_text(*message_id, text, Some(*offset))
            }
            Event::AssistantMessageCompleted {
                message_id, text, ..
            } if self.is_turn_thread(thread_id) => self.assistant_text(*message_id, text, None),
            Event::SteeringDelivered {
                message_id,
                text,
                images,
                ..
            } => self.push_user(Some(*message_id), text, images.len()),
            Event::ToolCallRequested { call } => {
                let tool = AppTranscriptTool {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    args: call.args.clone(),
                    effective_args: None,
                    status: "running".to_owned(),
                    result: None,
                    metadata: Value::Null,
                };
                if let Some(index) = self.running_subagent(thread_id) {
                    let tools = &mut self.entries[index]
                        .subagent
                        .as_mut()
                        .expect("subagent entry")
                        .tools;
                    if tools.len() < MAX_SUBAGENT_TOOLS {
                        tools.push(tool);
                    }
                    return Change::Updated(index);
                }
                self.stop_streaming();
                let key = self.allocate_key();
                self.entries.push(Entry {
                    key,
                    message_id: None,
                    role: Role::System,
                    text: "".into(),
                    images: 0,
                    tool: Some(tool),
                    subagent: None,
                    streaming: false,
                });
                Change::Pushed
            }
            Event::ToolCallResolved { call } => self.update_tool(&call.id, |tool| {
                tool.effective_args = Some(call.args.clone());
            }),
            Event::ApprovalRequested { call_id, .. } => self.update_tool(call_id, |tool| {
                tool.status = "waiting_approval".to_owned();
            }),
            Event::ApprovalResolved { call_id, approved } => self.update_tool(call_id, |tool| {
                tool.status = if *approved { "approved" } else { "denied" }.to_owned();
            }),
            Event::ToolFinished { result } => {
                self.update_tool(&result.call_id, |tool| finish_tool(tool, result))
            }
            Event::SubagentStarted {
                role,
                description,
                child_thread_id,
            } => {
                self.stop_streaming();
                let key = self.allocate_key();
                self.entries.push(Entry {
                    key,
                    message_id: None,
                    role: Role::System,
                    text: "".into(),
                    images: 0,
                    tool: None,
                    subagent: Some(AppTranscriptSubagent {
                        child_thread_id: child_thread_id.to_string(),
                        role: role.clone(),
                        description: description.clone(),
                        status: "running".to_owned(),
                        iterations: None,
                        tools: Vec::new(),
                    }),
                    streaming: false,
                });
                Change::Pushed
            }
            Event::SubagentFinished {
                status,
                iterations,
                child_thread_id,
                ..
            } => {
                let child = child_thread_id.to_string();
                let Some(index) = self.entries.iter().rposition(|entry| {
                    entry
                        .subagent
                        .as_ref()
                        .is_some_and(|subagent| subagent.child_thread_id == child)
                }) else {
                    return Change::None;
                };
                let subagent = self.entries[index]
                    .subagent
                    .as_mut()
                    .expect("subagent entry");
                subagent.status = status.clone();
                subagent.iterations = Some(*iterations);
                Change::Updated(index)
            }
            _ => Change::None,
        }
    }

    fn is_turn_thread(&self, thread_id: ThreadId) -> bool {
        self.turn_thread.is_none_or(|turn| turn == thread_id)
    }

    fn assistant_text(
        &mut self,
        message_id: MessageId,
        text: &str,
        offset: Option<usize>,
    ) -> Change {
        let completed = offset.is_none();
        if let Some(index) = self
            .entries
            .iter()
            .rposition(|entry| entry.message_id == Some(message_id))
        {
            let entry = &mut self.entries[index];
            entry.text = match offset {
                Some(offset) => splice_delta(&entry.text, offset, text),
                None if *entry.text == *text => entry.text.clone(),
                None => text.into(),
            };
            entry.streaming = !completed;
            return Change::Updated(index);
        }
        if text.is_empty() {
            return Change::None;
        }
        self.stop_streaming();
        let key = self.allocate_key();
        self.entries.push(Entry {
            key,
            message_id: Some(message_id),
            role: Role::Assistant,
            text: text.into(),
            images: 0,
            tool: None,
            subagent: None,
            streaming: !completed,
        });
        Change::Pushed
    }

    fn running_subagent(&self, thread_id: ThreadId) -> Option<usize> {
        let thread = thread_id.to_string();
        self.entries.iter().rposition(|entry| {
            entry.subagent.as_ref().is_some_and(|subagent| {
                subagent.status == "running" && subagent.child_thread_id == thread
            })
        })
    }

    fn update_tool(
        &mut self,
        call_id: &str,
        update: impl FnOnce(&mut AppTranscriptTool),
    ) -> Change {
        for (index, entry) in self.entries.iter_mut().enumerate().rev() {
            if let Some(tool) = entry.tool.as_mut().filter(|tool| tool.call_id == call_id) {
                update(tool);
                return Change::Updated(index);
            }
            if let Some(tool) = entry.subagent.as_mut().and_then(|subagent| {
                subagent
                    .tools
                    .iter_mut()
                    .rfind(|tool| tool.call_id == call_id)
            }) {
                update(tool);
                return Change::Updated(index);
            }
        }
        Change::None
    }

    fn stop_streaming(&mut self) {
        for entry in &mut self.entries {
            entry.streaming = false;
        }
    }

    fn allocate_key(&mut self) -> u64 {
        self.next_key += 1;
        self.next_key
    }

    fn entry_from(&mut self, message: AppTranscriptMessage) -> Entry {
        let role = match message.role.as_str() {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            _ => Role::System,
        };
        Entry {
            key: self.allocate_key(),
            message_id: message.message_id,
            role,
            text: message.text.into(),
            images: message.images.len(),
            tool: message.tool,
            subagent: message.subagent,
            streaming: message.streaming,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Identity {
    Message(MessageId),
    Tool(String),
    Subagent(String),
}

fn identity_of(entry: &Entry) -> Option<Identity> {
    if let Some(id) = entry.message_id {
        return Some(Identity::Message(id));
    }
    if let Some(tool) = &entry.tool {
        return Some(Identity::Tool(tool.call_id.clone()));
    }
    entry
        .subagent
        .as_ref()
        .map(|subagent| Identity::Subagent(subagent.child_thread_id.clone()))
}

fn splice_delta(current: &Arc<str>, offset: usize, delta: &str) -> Arc<str> {
    let keep = if offset <= current.len() && current.is_char_boundary(offset) {
        offset
    } else {
        current.len()
    };
    let mut text = String::with_capacity(keep + delta.len());
    text.push_str(&current[..keep]);
    text.push_str(delta);
    text.into()
}

fn finish_tool(tool: &mut AppTranscriptTool, result: &ToolResult) {
    tool.status = if result.ok { "done" } else { "failed" }.to_owned();
    tool.result = Some(result.text_or_status());
    tool.metadata = result.metadata.clone();
}

/// Whether a row renders the same after a snapshot replaced it.
fn same_row(before: &Entry, after: &Entry) -> bool {
    before.key == after.key
        && Arc::ptr_eq(&before.text, &after.text)
        && before.streaming == after.streaming
        && before.images == after.images
        && match (&before.tool, &after.tool) {
            (None, None) => true,
            (Some(a), Some(b)) => same_tool(a, b),
            _ => false,
        }
        && match (&before.subagent, &after.subagent) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.status == b.status
                    && a.iterations == b.iterations
                    && a.tools.len() == b.tools.len()
                    && a.tools.iter().zip(&b.tools).all(|(a, b)| same_tool(a, b))
            }
            _ => false,
        }
}

fn same_tool(a: &AppTranscriptTool, b: &AppTranscriptTool) -> bool {
    a.call_id == b.call_id
        && a.status == b.status
        && a.result == b.result
        && a.effective_args.is_some() == b.effective_args.is_some()
}

#[cfg(test)]
mod tests;

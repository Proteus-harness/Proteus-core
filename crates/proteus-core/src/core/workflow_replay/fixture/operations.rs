//! Validate nested ownership before any auxiliary operation becomes an oracle.
use crate::{
    contracts::{HookEvent, ModelCallOrigin, OperationOwner},
    core::{JournalEntry, JournalRecord, ToolCallRecordPhase},
    domain::{ExecutionId, ThreadId, TurnId},
};
use anyhow::{Result, anyhow, ensure};
use std::collections::{HashMap, HashSet};

struct Node {
    parent: OperationOwner,
    start: usize,
    end: usize,
    hook: bool,
}
fn key(owner: &OperationOwner) -> Option<String> {
    match owner {
        OperationOwner::Workflow => None,
        OperationOwner::Hook { invocation_id } => Some(format!("hook:{invocation_id}")),
        OperationOwner::Tool { call_id } => Some(format!("tool:{call_id}")),
    }
}

pub(super) fn validate(
    records: &[JournalRecord],
    execution: ExecutionId,
    thread: ThreadId,
    turn: TurnId,
) -> Result<()> {
    let selected = records
        .iter()
        .filter(|record| {
            record.execution_id == Some(execution)
                && record.thread_id == Some(thread)
                && record.turn_id == Some(turn)
        })
        .collect::<Vec<_>>();
    let mut nodes = HashMap::<String, Node>::new();
    let mut models = HashMap::new();
    for (position, record) in selected.iter().enumerate() {
        match &record.entry {
            JournalEntry::HookInvoked(trace) => {
                match &trace.input.event {
                    HookEvent::BeforeTool { call, .. } | HookEvent::AfterTool { call, .. } => {
                        ensure!(
                            trace.owner
                                == OperationOwner::Tool {
                                    call_id: call.id.clone()
                                },
                            "tool hook owner differs from its bound call"
                        )
                    }
                    HookEvent::BeforeModel { origin, .. } => ensure!(
                        *origin == ModelCallOrigin::Direct
                            && trace.owner == OperationOwner::Workflow,
                        "auxiliary generation must not repeat BeforeModel hooks"
                    ),
                    _ => ensure!(
                        trace.owner == OperationOwner::Workflow,
                        "root hook has non-workflow operation owner"
                    ),
                }
                for step in &trace.steps {
                    ensure!(
                        nodes
                            .insert(
                                format!("hook:{}", step.invocation_id),
                                Node {
                                    parent: trace.owner.clone(),
                                    start: position,
                                    end: position,
                                    hook: true
                                }
                            )
                            .is_none(),
                        "hook invocation identity was reused"
                    );
                }
            }
            JournalEntry::ToolCallRecorded(tool)
                if matches!(tool.phase, ToolCallRecordPhase::Requested { .. }) =>
            {
                let ToolCallRecordPhase::Requested { owner } = &tool.phase else {
                    unreachable!()
                };
                ensure!(
                    nodes
                        .insert(
                            format!("tool:{}", tool.call.id),
                            Node {
                                parent: owner.clone(),
                                start: position,
                                end: usize::MAX,
                                hook: false
                            }
                        )
                        .is_none(),
                    "tool invocation identity was reused"
                );
            }
            JournalEntry::ToolResultRecorded(result) | JournalEntry::ToolEffectRecorded(result) => {
                let node = nodes
                    .get_mut(&format!("tool:{}", result.result.call_id))
                    .ok_or_else(|| anyhow!("tool result has no selected operation request"))?;
                node.end = position;
            }
            JournalEntry::ModelRequestRecorded(request) => {
                ensure!(
                    (request.origin == ModelCallOrigin::Direct)
                        == (request.owner == OperationOwner::Workflow),
                    "model origin differs from its operation owner"
                );
                models.insert(request.exchange_id, (request.owner.clone(), position, None));
            }
            JournalEntry::ModelResponseRecorded(response) => {
                let model = models
                    .get_mut(&response.exchange_id)
                    .ok_or_else(|| anyhow!("model response has no selected operation request"))?;
                model.2 = Some(position);
            }
            _ => {}
        }
    }
    for (name, node) in &nodes {
        let mut owner = &node.parent;
        let mut seen = HashSet::from([name.clone()]);
        let mut ancestors = Vec::new();
        while let Some(parent_key) = key(owner) {
            ensure!(
                seen.insert(parent_key.clone()),
                "nested operation ownership cycle at {parent_key}"
            );
            let parent = nodes.get(&parent_key).ok_or_else(|| {
                anyhow!("nested operation references missing parent {parent_key}")
            })?;
            ancestors.push((parent_key, parent));
            owner = &parent.parent;
        }
        for (parent_key, parent) in ancestors {
            ensure!(
                node.end <= parent.end,
                "nested operation outlived parent {parent_key}"
            );
            if !parent.hook {
                ensure!(
                    node.start > parent.start,
                    "nested operation began before parent {parent_key}"
                );
            }
        }
        ensure!(
            node.end != usize::MAX,
            "nested tool operation has no settled effect or result"
        );
    }
    for (owner, position, terminal) in models.into_values() {
        let terminal =
            terminal.ok_or_else(|| anyhow!("model operation has no terminal outcome"))?;
        if let Some(parent_key) = key(&owner) {
            let parent = nodes
                .get(&parent_key)
                .ok_or_else(|| anyhow!("auxiliary model references missing parent {parent_key}"))?;
            ensure!(
                terminal <= parent.end
                    && position <= parent.end
                    && (parent.hook || position > parent.start),
                "auxiliary model falls outside parent operation"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        contracts::{
            ExecutionAttribution, HookInput, HookResponse, HookStep, HookStepOutcome, HookTrace,
        },
        core::{
            JOURNAL_SCHEMA_VERSION, ModelRequestRecorded, ModelResponseOutcome,
            ModelResponseRecorded, ToolCallRecorded, ToolResultRecorded,
        },
        domain::{
            ModelRef, ToolCall, ToolResult, new_execution_id, new_session_id, new_thread_id,
            new_turn_id,
        },
        model_standard::{CanonicalModelRequest, ModelFailure},
    };
    use uuid::Uuid;

    fn verify(entries: Vec<JournalEntry>) -> Result<()> {
        let execution = new_execution_id();
        let session = new_session_id();
        let thread = new_thread_id();
        let turn = new_turn_id();
        let records = entries
            .into_iter()
            .enumerate()
            .map(|(index, entry)| JournalRecord {
                schema_version: JOURNAL_SCHEMA_VERSION,
                record_id: Uuid::new_v4(),
                session_seq: index as u64,
                timestamp_ms: 0,
                session_id: session,
                execution_id: Some(execution),
                thread_id: Some(thread),
                turn_id: Some(turn),
                entry,
            })
            .collect::<Vec<_>>();
        validate(&records, execution, thread, turn)
    }
    fn hook(id: Uuid, owner: OperationOwner, event: HookEvent) -> JournalEntry {
        JournalEntry::HookInvoked(HookTrace {
            owner,
            input: HookInput {
                event: event.clone(),
                attribution: ExecutionAttribution::detached(new_execution_id()),
                cwd: ".".into(),
                conversation: None,
            },
            steps: vec![HookStep {
                invocation_id: id,
                module_id: "fixture".into(),
                outcome: HookStepOutcome::Accepted {
                    response: HookResponse::Continue,
                },
            }],
            output: Some(event),
        })
    }
    fn model(id: Uuid, owner: OperationOwner) -> JournalEntry {
        JournalEntry::ModelRequestRecorded(ModelRequestRecorded {
            exchange_id: id,
            origin: ModelCallOrigin::Auxiliary,
            owner,
            request: CanonicalModelRequest::new(ModelRef::new("fake", "fake"), vec![]),
        })
    }
    fn terminal(id: Uuid) -> JournalEntry {
        JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
            exchange_id: id,
            outcome: ModelResponseOutcome::Error {
                failure: ModelFailure::other("scripted"),
            },
        })
    }
    fn event() -> HookEvent {
        HookEvent::BeforeContext {
            input: crate::contracts::ContextPreparationInput::new(vec![], None),
            output: crate::contracts::ContextPreparationOutput::unchanged(vec![]),
            target: None,
        }
    }

    #[test]
    fn auxiliary_model_must_settle_before_its_hook_owner() {
        let invocation = Uuid::new_v4();
        let exchange = Uuid::new_v4();
        assert!(
            verify(vec![
                model(
                    exchange,
                    OperationOwner::Hook {
                        invocation_id: invocation
                    }
                ),
                terminal(exchange),
                hook(invocation, OperationOwner::Workflow, event())
            ])
            .is_ok()
        );
        assert!(
            verify(vec![
                model(
                    exchange,
                    OperationOwner::Hook {
                        invocation_id: invocation
                    }
                ),
                hook(invocation, OperationOwner::Workflow, event()),
                terminal(exchange)
            ])
            .is_err()
        );
    }
    #[test]
    fn missing_parent_and_owner_cycle_are_rejected_before_oracle_filtering() {
        let exchange = Uuid::new_v4();
        let invocation = Uuid::new_v4();
        assert!(
            verify(vec![
                model(
                    exchange,
                    OperationOwner::Hook {
                        invocation_id: invocation
                    }
                ),
                terminal(exchange)
            ])
            .is_err()
        );
        let call = ToolCall::new("cycle", "probe", serde_json::json!({}));
        let entries = vec![
            JournalEntry::ToolCallRecorded(ToolCallRecorded {
                call: call.clone(),
                phase: ToolCallRecordPhase::Requested {
                    owner: OperationOwner::Hook {
                        invocation_id: invocation,
                    },
                },
            }),
            hook(
                invocation,
                OperationOwner::Tool {
                    call_id: call.id.clone(),
                },
                HookEvent::BeforeTool {
                    call: call.clone(),
                    spec: None,
                    blocked: None,
                },
            ),
            JournalEntry::ToolResultRecorded(ToolResultRecorded {
                result: ToolResult::ok(call.id, "done"),
            }),
        ];
        assert!(verify(entries).unwrap_err().to_string().contains("cycle"));
    }
}

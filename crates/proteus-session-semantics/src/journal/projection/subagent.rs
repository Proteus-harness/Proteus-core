//! A durable child link is a reference to an admitted source call, not a tool
//! effect. A background child may publish it after that source turn settles.
use super::*;
use proteus_contracts::domain::{SubagentSessionBinding, ToolResult};

impl JournalValidationState {
    pub(super) fn validate_subagent_binding(
        &mut self,
        record: &JournalRecord,
        binding: &SubagentSessionBinding,
    ) -> Result<()> {
        anyhow::ensure!(
            binding.session_dir.is_absolute(),
            "subagent session directory must be absolute"
        );
        let owner = ExecutionFactOwner {
            execution_id: required_execution_id(record)?,
            thread_id: record.thread_id,
            turn_id: record.turn_id,
        };
        // Equality to the already validated source owner also proves its
        // execution/turn relationship, without reopening a settled execution.
        let lifecycle = self
            .tool_calls
            .get_mut(&binding.source_call_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "subagent binding has no preceding source call {}",
                    binding.source_call_id
                )
            })?;
        let lifecycle = Arc::make_mut(lifecycle);
        anyhow::ensure!(
            lifecycle.owner == Some(owner) && lifecycle.resolved && lifecycle.permitted,
            "subagent binding violates source call ownership or admission"
        );
        anyhow::ensure!(
            !lifecycle.binding_recorded,
            "duplicate subagent session binding for {}",
            binding.source_call_id
        );
        if let Some(reported) = &lifecycle.subagent_binding {
            anyhow::ensure!(
                reported.as_ref() == binding,
                "subagent binding disagrees with source result metadata"
            );
        }
        if lifecycle.subagent_binding.is_none() {
            lifecycle.subagent_binding = Some(Arc::new(binding.clone()));
        }
        lifecycle.binding_recorded = true;
        Ok(())
    }
}

pub(super) fn observe_result_binding(
    lifecycle: &mut ToolLifecycle,
    result: &ToolResult,
) -> Result<()> {
    let Some(session) = result.metadata.get("subagent_session") else {
        return Ok(());
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct SessionRef {
        session_id: SessionId,
        session_dir: std::path::PathBuf,
    }
    let session: SessionRef = serde_json::from_value(session.clone())?;
    let child_thread_id = serde_json::from_value(
        result
            .metadata
            .get("child_thread_id")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("subagent result metadata has no child thread"))?,
    )?;
    let reported = SubagentSessionBinding {
        source_call_id: result.call_id.clone(),
        child_thread_id,
        session_id: session.session_id,
        session_dir: session.session_dir,
    };
    anyhow::ensure!(
        reported.session_dir.is_absolute(),
        "subagent result directory must be absolute"
    );
    if let Some(binding) = &lifecycle.subagent_binding {
        anyhow::ensure!(
            binding.as_ref() == &reported,
            "source result metadata disagrees with subagent binding"
        );
    }
    if lifecycle.subagent_binding.is_none() {
        lifecycle.subagent_binding = Some(Arc::new(reported));
    }
    Ok(())
}

//! Acknowledged results retain their host-reserved canonical identities.
use super::*;

impl TurnScaffold {
    pub(super) fn retire_acknowledged_results(
        &mut self,
        messages: &[CanonicalMessage],
    ) -> Result<(), ProcessModuleError> {
        let mut completed = Vec::new();
        for (call_id, binding) in &self.result_bindings {
            if let Some((message, result)) = accepted_result(messages, call_id)? {
                if &binding.message(result.clone()) != message {
                    return Err(ProcessModuleError::new(
                        "checkpoint acknowledgement changed a reserved tool result identity",
                    ));
                }
                completed.push(call_id.clone());
            }
        }
        for call_id in completed {
            self.result_bindings.remove(&call_id);
        }
        Ok(())
    }

    pub(crate) fn append_tool_results(
        &mut self,
        results: impl IntoIterator<Item = ToolResult>,
    ) -> Result<(), ProcessModuleError> {
        for result in results {
            let call_id = result.call_id.clone();
            if let Some((message, accepted)) = accepted_result(&self.persistent_messages, &call_id)?
            {
                if !same_outcome(accepted, &result) {
                    return Err(ProcessModuleError::new(
                        "joined tool result differs from its accepted checkpoint outcome",
                    ));
                }
                if let Some(binding) = self.result_bindings.get(&call_id) {
                    if &binding.message(accepted.clone()) != message {
                        return Err(ProcessModuleError::new(
                            "accepted tool result changed its reserved identity",
                        ));
                    }
                }
                self.result_bindings.remove(&call_id);
                continue;
            }
            let message = match self.result_bindings.remove(&call_id) {
                Some(binding) => binding.message(result),
                None => crate::history::tool_result_message(result),
            };
            self.model_messages.push(message.clone());
            self.persistent_messages.push(message);
        }
        Ok(())
    }
}

fn accepted_result<'a>(
    messages: &'a [CanonicalMessage],
    call_id: &str,
) -> Result<Option<(&'a CanonicalMessage, &'a ToolResult)>, ProcessModuleError> {
    let mut matches = messages.iter().flat_map(|message| {
        message
            .parts
            .iter()
            .filter_map(move |part| match &part.payload {
                ContentPart::ToolResult { result } if result.call_id == call_id => {
                    Some((message, result))
                }
                _ => None,
            })
    });
    let result = matches.next();
    if matches.next().is_some() {
        return Err(ProcessModuleError::new(
            "accepted history has duplicate tool result calls",
        ));
    }
    Ok(result)
}
fn same_outcome(a: &ToolResult, b: &ToolResult) -> bool {
    let mut a = a.clone();
    let mut b = b.clone();
    for result in [&mut a, &mut b] {
        if let Some(metadata) = result.metadata.as_object_mut() {
            metadata.remove("duration_ms");
        }
    }
    a == b
}

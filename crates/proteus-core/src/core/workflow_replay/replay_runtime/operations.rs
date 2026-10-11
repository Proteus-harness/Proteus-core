//! Inject validated child-operation facts without invoking their implementations.
use super::*;

pub(super) fn consume_body(inner: &mut ReplayStateInner, owner: &OperationOwner) {
    consume(inner, owner, false);
}
pub(super) fn consume_hook(inner: &mut ReplayStateInner, invocation_id: uuid::Uuid) {
    consume(inner, &OperationOwner::Hook { invocation_id }, true);
}
fn consume(inner: &mut ReplayStateInner, owner: &OperationOwner, include_hooks: bool) {
    for (exchange, consumed) in &mut inner.auxiliary {
        if &exchange.owner == owner {
            *consumed = true;
        }
    }
    let children = inner
        .tools
        .iter()
        .enumerate()
        .filter(|(_, tool)| !tool.requested && &tool.recorded.owner == owner)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    for index in children {
        let call_id = inner.tools[index].recorded.call.id.clone();
        let tool = &mut inner.tools[index];
        tool.requested = true;
        tool.approval_requested = tool.recorded.approval_reason.is_some();
        tool.resolved = true;
        tool.result_recorded = true;
        inner
            .actual_to_expected
            .insert(call_id.clone(), call_id.clone());
        inner
            .expected_to_actual
            .insert(call_id.clone(), call_id.clone());
        consume(inner, &OperationOwner::Tool { call_id }, true);
    }
    if include_hooks {
        let traces = inner
            .hooks
            .iter()
            .enumerate()
            .filter(|(_, (trace, consumed))| !consumed && &trace.owner == owner)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        for index in traces {
            inner.hooks[index].1 = true;
            let invocations = inner.hooks[index]
                .0
                .steps
                .iter()
                .map(|step| step.invocation_id)
                .collect::<Vec<_>>();
            for invocation in invocations {
                consume_hook(inner, invocation);
            }
        }
    }
}

pub(super) fn owners_equal(
    actual: &OperationOwner,
    expected: &OperationOwner,
    ids: &HashMap<CallId, CallId>,
) -> bool {
    match (actual, expected) {
        (OperationOwner::Workflow, OperationOwner::Workflow) => true,
        (OperationOwner::Hook { invocation_id: a }, OperationOwner::Hook { invocation_id: b }) => {
            a == b
        }
        (OperationOwner::Tool { call_id: a }, OperationOwner::Tool { call_id: b }) => {
            ids.get(a).unwrap_or(a) == b
        }
        _ => false,
    }
}

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};
use uuid::Uuid;

use super::projection::ExecutionFactOwner;

/// Context builds share the execution recorder's attribution and have no
/// caller-selected operation owner or conversation requirement.
#[derive(Debug, Clone, Default)]
pub(super) struct ContextLifecycle {
    requests: HashMap<Uuid, ExecutionFactOwner>,
    responses: HashSet<Uuid>,
}

impl ContextLifecycle {
    pub(super) fn request(&mut self, invocation_id: Uuid, owner: ExecutionFactOwner) -> Result<()> {
        if self.requests.insert(invocation_id, owner).is_some() {
            bail!("duplicate context build request {invocation_id}");
        }
        Ok(())
    }

    pub(super) fn response(
        &mut self,
        invocation_id: Uuid,
        owner: ExecutionFactOwner,
    ) -> Result<()> {
        let Some(request_owner) = self.requests.get(&invocation_id) else {
            bail!("context response {invocation_id} has no preceding request");
        };
        if *request_owner != owner {
            bail!("context response {invocation_id} changed lifecycle owner");
        }
        if !self.responses.insert(invocation_id) {
            bail!("duplicate context build response {invocation_id}");
        }
        Ok(())
    }

    pub(super) fn interrupted(&self) -> Vec<Uuid> {
        let mut pending = self
            .requests
            .keys()
            .filter(|invocation_id| !self.responses.contains(invocation_id))
            .copied()
            .collect::<Vec<_>>();
        pending.sort_unstable();
        pending
    }
}

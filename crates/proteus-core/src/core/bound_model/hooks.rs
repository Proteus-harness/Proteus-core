use super::BoundModel;
use crate::contracts::HookEventKind;
use crate::core::model_call_scope::current_model_call_origin;
use crate::model_standard::{CanonicalModelRequest, ModelOperation};
use anyhow::Result;

impl BoundModel {
    pub(super) async fn apply_before_model(
        &self,
        request: CanonicalModelRequest,
    ) -> Result<CanonicalModelRequest> {
        if let Some(tools) = &self.tool_authority {
            tools.validate_model_tools(&request.tools, &self.hook_cwd)?;
        }
        let mut request = self.service.prepare_request(request)?;
        self.binding.bind_request(&mut request)?;
        // Native compaction needs the same configured instruction transforms as
        // a direct request. Ordinary auxiliary generations still bypass hooks,
        // so model calls made by BeforeModel hooks do not recursively re-enter.
        if current_model_call_origin() == crate::contracts::ModelCallOrigin::Auxiliary
            && request.operation != ModelOperation::Compact
        {
            return Ok(request);
        }
        let conversation = if self.hooks.is_active_for(HookEventKind::BeforeModel) {
            match &self.binding.conversation {
                Some(reader) => Some(reader.read().await?),
                None => None,
            }
        } else {
            None
        };
        let event = self
            .hooks
            .apply(crate::contracts::HookInput {
                event: crate::contracts::HookEvent::BeforeModel {
                    origin: current_model_call_origin(),
                    request,
                },
                attribution: self.hook_attribution,
                cwd: self.hook_cwd.clone(),
                conversation,
            })
            .await?;
        let crate::contracts::HookEvent::BeforeModel {
            request: transformed,
            ..
        } = event
        else {
            anyhow::bail!("hook changed model event kind");
        };
        let mut request = self.service.prepare_request(transformed)?;
        self.binding.bind_request(&mut request)?;
        if let Some(tools) = &self.tool_authority {
            tools.validate_model_tools(&request.tools, &self.hook_cwd)?;
        }
        Ok(request)
    }
}

//! Profile publication shared by UI saves, explicit reload and file refresh.
use super::{AppServerEvent, AppServerHandle, prepare_assembly};
use crate::{
    core::{PreparedAssembly, RuntimeReloadReport, SessionConfigSnapshot},
    domain::PermissionMode,
};
use anyhow::Result;

impl AppServerHandle {
    pub(super) async fn publish_profile<F, Fut>(
        &self,
        assembly: PreparedAssembly,
        explicit_mode: Option<PermissionMode>,
        commit: F,
    ) -> Result<RuntimeReloadReport>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let _transition = self.settings_transition.lock().await;
        let config = assembly.plan().shared_config();
        let previous = self.config.read().await.clone();
        let prior_behavior = self.behavior.read().await.clone();
        let behavior = super::session_behavior::resolved(prior_behavior.clone(), &config);
        let behavior_active = behavior != Default::default();
        let assembly = if behavior_active {
            Box::pin(prepare_assembly(
                super::session_behavior::overlay(&config, &behavior).into(),
                &self.cwd,
                self.config_path.as_deref(),
            ))
            .await?
        } else {
            assembly
        };
        let scoped = super::session_model::stored(self, &config).await?;
        let invalidated = match &scoped {
            super::session_model::Stored::Invalidated => true,
            super::session_model::Stored::Absent => match self.config_path.as_deref() {
                Some(path) => super::session_selection::binding_changed(&previous, &config, path)?,
                None => false,
            },
            super::session_model::Stored::Current(_) => false,
        };
        let scoped_model = match &scoped {
            super::session_model::Stored::Current(selected) => Some(selected.model.clone()),
            _ => None,
        };
        let scoped_reasoning = match &scoped {
            super::session_model::Stored::Current(selected) => Some(selected.reasoning.clone()),
            _ => None,
        };
        let assembly = match &scoped {
            super::session_model::Stored::Current(selected) => {
                super::session_model::prepare(self, assembly.plan().config(), selected).await?
            }
            _ => assembly,
        };
        let effective = assembly.plan().config();
        let old_model = previous.selected_model_config()?;
        let next_model = config.selected_model_config()?;
        let provider_changed = config.active_provider != previous.active_provider;
        let model_changed = provider_changed
            || old_model.as_ref().map(|model| model.model_ref())
                != next_model.as_ref().map(|model| model.model_ref());
        let model_ref = if scoped_model.is_some() {
            scoped_model
        } else if model_changed || invalidated {
            next_model.as_ref().map(|model| model.model_ref())
        } else {
            None
        };
        let reasoning_changed = provider_changed
            || old_model.as_ref().map(|model| &model.reasoning)
                != next_model.as_ref().map(|model| &model.reasoning);
        let reasoning = if scoped_reasoning.is_some() {
            scoped_reasoning
        } else if reasoning_changed || invalidated {
            next_model.as_ref().map(|model| model.reasoning.clone())
        } else {
            None
        };
        let mode = if behavior_active || prior_behavior != Default::default() {
            Some(effective.permissions.mode)
        } else {
            explicit_mode.or_else(|| {
                (previous.permissions.mode != config.permissions.mode)
                    .then_some(config.permissions.mode)
            })
        };
        let snapshot = SessionConfigSnapshot::from_runtime_config(
            effective,
            assembly.registry(),
            mode.unwrap_or(self.permission_mode().await),
            self.runtime.approval_available(),
        );
        let report = self
            .runtime
            .reload_assembly_with_commit(
                assembly,
                Some(snapshot),
                model_ref,
                mode,
                reasoning,
                || {
                    super::session_behavior::commit_profile(
                        self,
                        prior_behavior.selected.is_some() && behavior.selected.is_none(),
                        commit,
                    )
                },
            )
            .await?;
        *self.config.write().await = config;
        *self.behavior.write().await = behavior;
        self.set_profile_error(None).await;
        let _ = self.events.send(AppServerEvent::ModulesReloaded {
            old_epoch: report.old_epoch,
            new_epoch: report.new_epoch,
            tool_names: report.tool_names.clone(),
        });
        Ok(report)
    }

    pub(super) async fn set_profile_error(&self, error: Option<String>) {
        let mut stored = self.profile_error.lock().await;
        if *stored != error {
            *stored = error.clone();
            let _ = self
                .events
                .send(AppServerEvent::ProfileReloadStatus { error });
        }
    }

    pub async fn reload_tools(&self) -> Result<RuntimeReloadReport> {
        let lock = self
            .config_path
            .as_deref()
            .map(super::config_builder::path_lock)
            .transpose()?;
        let _guard = match lock {
            Some(lock) => Some(lock.lock_owned().await),
            None => None,
        };
        let config = super::reload_tools_config(self.config_path.as_deref(), &self.config).await?;
        let assembly = prepare_assembly(config, &self.cwd, self.config_path.as_deref()).await?;
        self.publish_profile(assembly, None, || async { Ok(()) })
            .await
    }
}

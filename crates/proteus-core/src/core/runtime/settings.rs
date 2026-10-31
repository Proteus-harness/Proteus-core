use super::*;

impl AgentRuntime {
    pub(crate) async fn configuration_view(
        &self,
    ) -> (
        super::RuntimeSnapshot,
        Option<crate::domain::ModelRef>,
        crate::domain::ReasoningConfig,
        crate::domain::PermissionMode,
    ) {
        let state = self.capture_execution_snapshot().await;
        (
            state.runtime,
            state.model_ref,
            state.reasoning,
            state.permission_mode,
        )
    }
    pub async fn model_quota(&self) -> Result<Option<crate::contracts::ModelQuotaSnapshot>> {
        let snapshot = self.snapshot().await;
        let quota = snapshot.registry.model_quota().await?;
        anyhow::ensure!(
            self.snapshot().await.epoch == snapshot.epoch,
            "model provider changed during quota lookup; retry lookup"
        );
        Ok(quota)
    }

    pub async fn model_catalog(&self) -> Result<Option<crate::contracts::ModelCatalog>> {
        self.snapshot().await.registry.model_catalog().await
    }

    pub async fn set_model_name(&self, model: String) -> Result<()> {
        self.set_model_name_with_commit(model, |_, _, _| async { Ok(()) })
            .await
    }

    pub(crate) async fn set_model_name_with_commit<F, Fut>(
        &self,
        model: String,
        commit: F,
    ) -> Result<()>
    where
        F: FnOnce(AppConfig, ModelRef, ReasoningConfig) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let _guard = self.services.reload_lock.lock().await;
        let model = model.trim();
        anyhow::ensure!(!model.is_empty(), "model name must not be empty");
        let admission = self.capture_execution_snapshot().await;
        let snapshot = admission.runtime;
        let mut model_ref = admission
            .model_ref
            .ok_or_else(|| anyhow::anyhow!("no active model is configured"))?;
        model_ref.model = model.to_owned();
        let catalog = snapshot.registry.model_catalog().await?;
        let selected = catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .models
                    .iter()
                    .find(|entry| entry.id == model)
                    .ok_or_else(|| {
                        anyhow::anyhow!("model {model:?} is absent from the provider catalog")
                    })
            })
            .transpose()?;
        let tools = snapshot.registry.tools_for_model(Some(&model_ref))?;
        let mut reasoning = admission.reasoning;
        if let Some(selected) = selected {
            if reasoning
                .effort
                .as_ref()
                .is_none_or(|effort| !selected.reasoning_efforts.contains(effort))
            {
                reasoning.effort = selected.default_reasoning_effort.clone();
            }
            if reasoning.effort.as_deref() == Some("none") || selected.reasoning_efforts.is_empty()
            {
                reasoning.summary = false;
                reasoning.budget_tokens = None;
            }
        }
        commit(
            snapshot.assembly_plan.config().clone(),
            model_ref.clone(),
            reasoning.clone(),
        )
        .await?;
        let mut state = self.services.execution_state.write().await;
        state.model_ref = Some(model_ref);
        state.runtime.registry.tools = tools;
        state.reasoning = reasoning;
        Ok(())
    }

    /// Полная замена provider+model, например после смены `active_provider`
    /// через config builder: `reload_assembly` пересобирает model adapter, но
    /// не трогает runtime override model_ref.
    pub async fn set_model_ref(&self, model_ref: ModelRef) -> Result<()> {
        let _guard = self.services.reload_lock.lock().await;
        let snapshot = self.snapshot().await;
        let tools = snapshot.registry.tools_for_model(Some(&model_ref))?;
        let mut state = self.services.execution_state.write().await;
        anyhow::ensure!(
            state.runtime.epoch == snapshot.epoch,
            "model provider changed during selection; retry selection"
        );
        state.model_ref = Some(model_ref);
        state.runtime.registry.tools = tools;
        Ok(())
    }

    pub async fn model_ref(&self) -> Option<ModelRef> {
        self.services.execution_state.read().await.model_ref.clone()
    }

    pub async fn set_reasoning_enabled(&self, enabled: bool) {
        // This no-I/O wrapper cannot fail for a configured runtime.
        self.set_reasoning_enabled_with_commit(enabled, |_, _, _| async { Ok(()) })
            .await
            .expect("in-memory reasoning update");
    }

    pub(crate) async fn set_reasoning_enabled_with_commit<F, Fut>(
        &self,
        enabled: bool,
        commit: F,
    ) -> Result<()>
    where
        F: FnOnce(AppConfig, ModelRef, ReasoningConfig) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let _guard = self.services.reload_lock.lock().await;
        let state = self.services.execution_state.read().await;
        let default_reasoning = state.configured_reasoning();
        let mut reasoning = state.reasoning.clone();
        if enabled {
            if reasoning.effort.is_none() || reasoning.effort.as_deref() == Some("none") {
                reasoning.effort = default_reasoning.effort;
            }
            reasoning.summary = default_reasoning.summary;
            reasoning.budget_tokens = default_reasoning.budget_tokens;
        } else {
            reasoning.effort = None;
            reasoning.summary = false;
            reasoning.budget_tokens = None;
        }
        let model = state.model_ref.clone();
        let config = state.runtime.assembly_plan.config().clone();
        drop(state);
        if let Some(model) = model {
            commit(config, model, reasoning.clone()).await?;
        }
        self.services.execution_state.write().await.reasoning = reasoning;
        Ok(())
    }

    pub async fn set_reasoning_effort(&self, effort: Option<String>) -> Result<()> {
        self.set_reasoning_effort_with_commit(effort, |_, _, _| async { Ok(()) })
            .await
    }

    pub(crate) async fn set_reasoning_effort_with_commit<F, Fut>(
        &self,
        effort: Option<String>,
        commit: F,
    ) -> Result<()>
    where
        F: FnOnce(AppConfig, ModelRef, ReasoningConfig) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let _guard = self.services.reload_lock.lock().await;
        let snapshot = self.snapshot().await;
        let catalog = snapshot.registry.model_catalog().await?;
        let state = self.services.execution_state.read().await;
        anyhow::ensure!(
            state.runtime.epoch == snapshot.epoch,
            "model provider changed during catalog lookup; retry selection"
        );
        if let (Some(catalog), Some(effort)) = (&catalog, &effort) {
            let model = catalog
                .models
                .iter()
                .find(|entry| {
                    Some(entry.id.as_str()) == state.model_ref.as_ref().map(|m| m.model.as_str())
                })
                .ok_or_else(|| {
                    anyhow::anyhow!("active model is absent from the provider catalog")
                })?;
            anyhow::ensure!(
                model.reasoning_efforts.contains(effort),
                "unsupported reasoning effort {effort:?} for {}",
                model.id
            );
        }
        let default_reasoning = state.configured_reasoning();
        let mut reasoning = state.reasoning.clone();
        match effort.as_deref() {
            // «none» — первоклассное значение effort: выключает рассуждения
            // целиком. Веб-клиент шлёт его вместо пары /reasoning + /effort.
            Some("none") => {
                reasoning.effort = Some("none".to_owned());
                reasoning.summary = false;
                reasoning.budget_tokens = None;
            }
            // Конкретный effort включает рассуждения, даже если они были
            // выключены: summary/budget возвращаются к дефолтам конфига.
            Some(value) => {
                if !reasoning.is_enabled() {
                    reasoning.summary = default_reasoning.summary;
                    reasoning.budget_tokens = default_reasoning.budget_tokens;
                }
                reasoning.effort = Some(value.to_owned());
            }
            // null — «auto»: явного effort нет, остальное не трогаем.
            None => reasoning.effort = None,
        }
        let model = state.model_ref.clone();
        let config = state.runtime.assembly_plan.config().clone();
        drop(state);
        if let Some(model) = model {
            commit(config, model, reasoning.clone()).await?;
        }
        self.services.execution_state.write().await.reasoning = reasoning;
        Ok(())
    }

    /// Restore a previously validated user choice before exposing the runtime.
    /// No catalog request is required: a temporary discovery outage must not
    /// replace a durable selection with the configured default.
    pub(crate) async fn restore_model_selection(
        &self,
        model: ModelRef,
        reasoning: ReasoningConfig,
    ) -> Result<()> {
        let _guard = self.services.reload_lock.lock().await;
        let snapshot = self.snapshot().await;
        let configured = snapshot
            .registry
            .model_config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("saved selection has no active model provider"))?;
        anyhow::ensure!(
            configured.provider == model.provider,
            "saved model provider does not match active provider"
        );
        let tools = snapshot.registry.tools_for_model(Some(&model))?;
        let mut state = self.services.execution_state.write().await;
        state.model_ref = Some(model);
        state.reasoning = reasoning;
        state.runtime.registry.tools = tools;
        Ok(())
    }

    pub async fn reasoning(&self) -> ReasoningConfig {
        self.services.execution_state.read().await.reasoning.clone()
    }
}

impl RuntimeExecutionState {
    fn configured_reasoning(&self) -> ReasoningConfig {
        // Defaults belong to the published assembly, not the runtime's first
        // provider. Reading them under execution_state preserves reload rollback.
        self.runtime
            .registry
            .model_config
            .as_ref()
            .map(|config| config.reasoning.clone())
            .unwrap_or_default()
    }
}

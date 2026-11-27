use std::{path::PathBuf, sync::Arc};

use anyhow::Result;

mod config_schemas;

use crate::{
    contracts::{
        AgentControl, AgentWorkflowContext, ApprovalPolicy, ContextBuilder, EventEmitter,
        ExecutionContext, Model, SessionStorage, ToolRegistry, UserInputTransport, Workflow,
    },
    core::{
        AgentControlRuntime, AppConfig, AssemblyPlan, BoundModel, ModeAwarePolicy,
        ModelExecutionBinding, ModelService, ModuleBuildContext, ModuleCatalog, PolicyBuildContext,
        PreparedAssembly,
    },
    domain::{ModelRef, ReasoningConfig, SessionId, ThreadId, TurnId},
    stubs::{DenyAllPolicy, EmptyContextBuilder, NoWorkflow},
};

#[cfg(test)]
#[path = "registry/compaction_tests.rs"]
mod compaction_tests;
#[cfg(test)]
#[path = "registry/discovery_tests.rs"]
mod discovery_tests;
#[cfg(test)]
#[path = "tool_terminal_binding_tests.rs"]
mod tool_terminal_tests;

#[derive(Clone)]
pub struct RuntimeRegistry {
    // Immutable per registry generation, also retained by schema discovery tasks.
    config_exports: Arc<Vec<crate::process_adapters::ProcessExportConfig>>,
    pub hooks: Vec<(String, Arc<dyn crate::contracts::HookHandler>)>,
    hook_subscriptions: super::config::HookSubscriptions,
    pub cwd: PathBuf,
    pub model_config: Option<crate::core::ModelConfig>,
    pub runtime_config: crate::core::RuntimeConfig,
    pub instructions: Vec<crate::model_standard::InstructionBlock>,
    model_service: Option<Arc<ModelService>>,
    pub context: Arc<dyn ContextBuilder>,
    pub tools: ToolRegistry,
    tool_allowlist: Option<std::collections::BTreeSet<String>>,
    /// Description-only inventory, shared cheaply by snapshots of this generation.
    pub(crate) tool_inventory: Arc<Vec<(crate::contracts::ToolSource, crate::domain::ToolSpec)>>,
    pub(crate) process_tool_specs: Vec<(crate::contracts::ToolSource, crate::domain::ToolSpec)>,
    pub policy: Arc<dyn ApprovalPolicy>,
    pub agent_control: Option<Arc<dyn AgentControl>>,
    pub workflow: Arc<dyn Workflow>,
    pub session_storage: Option<Arc<dyn SessionStorage>>,
    pub(crate) context_providers: Vec<(String, Arc<dyn crate::core::RepoAwareContextProvider>)>,
    pub(crate) mcp_servers: Vec<proteus_contracts::app_protocol::addons::AppMcpServerState>,
    pub(crate) plugins: Vec<proteus_contracts::app_protocol::addons::AppAgentPluginState>,
}

impl RuntimeRegistry {
    pub(crate) fn bind_context(&self, binding: &ModelExecutionBinding) -> Arc<dyn ContextBuilder> {
        Arc::new(super::bound_context::BoundContext::new(
            self.context.clone(),
            binding.scope().clone(),
            binding.attribution(),
            binding.recorder(),
            self.runtime_config.context_timeout_ms,
        ))
    }

    pub(crate) fn bind_hooks(
        &self,
        scope: crate::contracts::ExecutionScope,
        recorder: Arc<dyn crate::contracts::ExecutionRecorder>,
    ) -> Arc<dyn crate::contracts::ExecutionHooks> {
        Arc::new(
            crate::core::RuntimeHookChain::new(
                self.hooks.clone(),
                scope,
                recorder,
                self.model_service.clone(),
            )
            .with_subscriptions(self.hook_subscriptions.clone()),
        )
    }
    pub(crate) async fn model_quota(&self) -> Result<Option<crate::contracts::ModelQuotaSnapshot>> {
        match &self.model_service {
            Some(model) => model.quota().await,
            None => Ok(None),
        }
    }

    pub(crate) async fn model_catalog(&self) -> Result<Option<crate::contracts::ModelCatalog>> {
        match &self.model_service {
            Some(model) => model.catalog().await,
            None => Ok(None),
        }
    }

    pub(crate) async fn release_idle_component_workers(&self) -> Result<usize> {
        use futures_util::stream::{FuturesUnordered, StreamExt};

        if !self
            .config_exports
            .iter()
            .any(|export| export.releases_when_idle())
        {
            return Ok(0);
        }
        let workspace = self.cwd.canonicalize()?;
        let mut seen = std::collections::HashSet::new();
        let mut workers = FuturesUnordered::new();
        for export in self.config_exports.iter() {
            if export.releases_when_idle() && seen.insert(export.component_id()) {
                workers.push(export.release_idle_broker(&workspace));
            }
        }
        let mut released = 0;
        let mut first_error = None;
        while let Some(result) = workers.next().await {
            match result {
                Ok(true) => released += 1,
                Ok(false) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(released), Err)
    }

    /// Reuse the configured component's metadata path without preparing an
    /// assembly, binding a model, or calling the provider's describe method.
    pub(crate) async fn inactive_model_catalog(
        &self,
        provider: &str,
    ) -> Result<Option<crate::contracts::ModelCatalog>> {
        use crate::contracts::PROCESS_MODEL_CONTRACT_VERSION;
        use crate::process_adapters::ProcessExportClient;

        let Some(export) = self
            .config_exports
            .iter()
            .find(|export| export.slot() == "model" && export.module_id() == provider)
            .cloned()
        else {
            return Ok(None);
        };
        let cwd = self.cwd.clone();
        let timeout = self.runtime_config.model_timeout_ms;
        let client = tokio::task::spawn_blocking(move || {
            ProcessExportClient::connect(
                "model",
                PROCESS_MODEL_CONTRACT_VERSION,
                export,
                &cwd,
                timeout,
            )
        })
        .await??;
        client.model_catalog().await
    }

    pub(crate) fn model_capabilities(
        &self,
        model: &ModelRef,
    ) -> Result<Option<crate::model_standard::ModelCapabilities>> {
        self.model_service
            .as_ref()
            .map(|service| service.capabilities(model))
            .transpose()
    }

    pub fn from_config(config: &AppConfig, cwd: PathBuf) -> Result<Self> {
        Ok(PreparedAssembly::from_config(config.clone(), cwd, None)?
            .into_parts()
            .1)
    }

    pub fn from_catalog(config: &AppConfig, cwd: PathBuf, catalog: ModuleCatalog) -> Result<Self> {
        Ok(
            PreparedAssembly::from_catalog(config.clone(), cwd, None, catalog)?
                .into_parts()
                .1,
        )
    }

    pub(crate) fn from_plan(plan: &AssemblyPlan, catalog: ModuleCatalog) -> Result<Self> {
        Self::from_plan_with_storage(plan, catalog, None)
    }

    pub(crate) fn from_plan_with_storage(
        plan: &AssemblyPlan,
        catalog: ModuleCatalog,
        storage: Option<super::StorageService>,
    ) -> Result<Self> {
        plan.ensure_valid()?;
        let config = plan.config();
        let cwd = plan.cwd();
        let addons = crate::core::agent_plugins::resolve(&config.addons, cwd);
        let context_providers = catalog.build_context_providers(cwd, &addons.skills)?;
        let build_ctx = ModuleBuildContext {
            config,
            cwd,
            context_providers: &context_providers,
        };
        let hooks = catalog.build_hooks(&config.modules.hooks, &build_ctx)?;
        let model_config = config.selected_model_config()?;
        let model_service = model_config
            .as_ref()
            .map(|cfg| {
                catalog
                    .build_model_adapter(cfg, cwd)
                    .map(|model| Arc::new(ModelService::new(model)))
            })
            .transpose()?;

        let context: Arc<dyn ContextBuilder> =
            match plan.module_id(crate::domain::ModuleKind::Context) {
                Some(id) => catalog.build_context(id, &build_ctx)?,
                None => Arc::new(EmptyContextBuilder),
            };
        let agent_control_runtime = AgentControlRuntime::from_config(&config.agent_control)?;
        let agent_control = agent_control_runtime.service();
        let surface = catalog.build_tools(&build_ctx, &addons.skills, &addons.servers)?;
        let mut tools = surface.tools;
        agent_control_runtime.register_tools(&mut tools, config.runtime.workflow_timeout_ms)?;
        if let (Some(service), Some(config)) = (&model_service, &model_config) {
            crate::core::register_provider_hosted_tools(
                &mut tools,
                service.id().as_ref(),
                service.provider_hosted_tools(&config.model_ref())?,
            )?;
        }

        let mut tool_inventory = tools.entries();
        use crate::contracts::ToolProvider;
        for provided in crate::tools::BuiltinToolProvider::new(vec![
            "request_user_input".into(),
            "AskUserQuestion".into(),
        ])
        .tools()?
        {
            let spec = provided.tool.spec();
            if !tool_inventory
                .iter()
                .any(|(_, existing)| existing.name == spec.name)
            {
                tool_inventory.push((provided.source, spec));
            }
        }
        for (source, spec) in &surface.process_tool_specs {
            if !tool_inventory
                .iter()
                .any(|(_, existing)| existing.name == spec.name)
            {
                tool_inventory.push((source.clone(), spec.clone()));
            }
        }
        let tool_inventory = Arc::new(tool_inventory);
        let tool_allowlist = config.behavior_tools.as_ref().map(|names| {
            names
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
        });
        if let Some(names) = &tool_allowlist {
            for name in names {
                anyhow::ensure!(
                    tools.get(name).is_some(),
                    "unsupported behavior mode tool {name:?}"
                );
            }
            tools = filter_tools(&tools, names)?;
        }

        let policy_ctx = PolicyBuildContext { cwd };
        let policy: Arc<dyn ApprovalPolicy> =
            match plan.module_id(crate::domain::ModuleKind::Policy) {
                Some(id) => catalog.build_policy(id, &policy_ctx)?,
                None => Arc::new(DenyAllPolicy),
            };
        let workflow: Arc<dyn Workflow> = match plan.module_id(crate::domain::ModuleKind::Workflow)
        {
            Some(id) => catalog.build_workflow(id, &build_ctx)?,
            None => Arc::new(NoWorkflow),
        };
        let session_storage = match storage {
            Some(service) => Some(service),
            None => plan
                .module_id(crate::domain::ModuleKind::SessionStorage)
                .map(|id| catalog.build_session_storage(id, &build_ctx))
                .transpose()?,
        };
        // Bootstrap validates every process tool and records its inventory,
        // including disabled entries. Once all runtime slots are assembled,
        // only components retained by a live adapter need to remain running.
        // Config discovery can reconnect on demand using the same descriptors.
        for export in &catalog.config_exports {
            export.release_discovery_brokers()?;
        }
        Ok(Self {
            config_exports: Arc::new(catalog.config_exports),
            session_storage,
            hooks,
            hook_subscriptions: config.modules.hook_events.clone(),
            cwd: cwd.to_path_buf(),
            model_config,
            runtime_config: config.runtime.clone(),
            instructions: config.instruction_blocks(),
            model_service,
            context,
            tools,
            tool_allowlist,
            tool_inventory,
            process_tool_specs: surface.process_tool_specs,
            policy,
            agent_control,
            workflow,
            context_providers,
            mcp_servers: surface.mcp_servers,
            plugins: addons.plugins,
        })
    }

    pub fn execution_context(
        &self,
        model_binding: ModelExecutionBinding,
        approval: Arc<dyn crate::contracts::ApprovalTransport>,
        permission_mode: crate::domain::PermissionMode,
    ) -> Result<ExecutionContext> {
        let model_ref = self.model_config.as_ref().map(|cfg| cfg.model_ref());
        self.execution_context_for_model(
            model_binding,
            approval,
            permission_mode,
            model_ref.as_ref(),
        )
    }

    pub fn execution_context_for_model(
        &self,
        model_binding: ModelExecutionBinding,
        approval: Arc<dyn crate::contracts::ApprovalTransport>,
        permission_mode: crate::domain::PermissionMode,
        model_ref: Option<&ModelRef>,
    ) -> Result<ExecutionContext> {
        let scope = model_binding.scope().clone();
        let attribution = model_binding.attribution();
        let chain = Arc::new(
            crate::core::RuntimeHookChain::new(
                self.hooks.clone(),
                scope.clone(),
                model_binding.recorder(),
                self.model_service.clone(),
            )
            .with_subscriptions(self.hook_subscriptions.clone()),
        );
        let hooks: Arc<dyn crate::contracts::ExecutionHooks> = chain.clone();
        let weak_hooks: Arc<dyn crate::contracts::ExecutionHooks> = Arc::new(
            super::extension_operations::WeakExecutionHooks(Arc::downgrade(&hooks)),
        );
        let policy: Arc<dyn ApprovalPolicy> =
            Arc::new(ModeAwarePolicy::new(permission_mode, self.policy.clone()));
        let selected_tools = self.tools_for_model(model_ref)?;
        let permission_grants: Arc<crate::contracts::ExecutionPermissionGrants> = Arc::default();
        let origin = match attribution.agent {
            Some(agent) => crate::contracts::RequestOrigin::for_turn(
                scope.execution_id,
                agent.thread_id,
                agent.turn_id,
            ),
            None => crate::contracts::RequestOrigin::for_execution(scope.execution_id),
        };
        let tool_binding = crate::core::ToolExecutionBinding::attributed(
            scope.clone(),
            attribution,
            origin,
            model_binding.tool_recorder(),
        )
        .with_terminal_factory(model_binding.tool_terminal_factory())
        .with_observer(model_binding.tool_observer());
        let tools = crate::core::BoundTools::new(
            selected_tools.clone(),
            policy.clone(),
            approval.clone(),
            permission_grants.clone(),
            tool_binding,
        )
        .with_conversation(
            model_binding.conversation_reader(),
            attribution.agent.map(|agent| agent.session_id),
        );
        let auxiliary_model = self.model_service.as_ref().map(|service| {
            Arc::new(
                BoundModel::new(
                    service.clone(),
                    model_binding.clone(),
                    self.runtime_config.model_timeout_ms,
                )
                .with_tool_authority(tools.clone())
                .with_hooks(
                    // Compact needs the configured transforms even from a hook
                    // callback. Keep this back-reference weak: the chain owns
                    // the auxiliary model through its operation facade.
                    weak_hooks.clone(),
                    attribution,
                    self.cwd.clone(),
                ),
            ) as Arc<dyn Model>
        });
        let operations =
            Arc::new_cyclic(|weak| super::extension_operations::InvocationOperations {
                operations: Some(super::extension_operations::ExtensionOperations {
                    model: auxiliary_model.clone(),
                    tools: tools
                        .clone()
                        .with_hooks(weak_hooks)
                        .with_operations(Some(Arc::new(
                            super::extension_operations::WeakToolHost(weak.clone()),
                        ))),
                }),
                owner: crate::contracts::OperationOwner::Workflow,
                lineage: vec![],
                cancellation: scope.cancellation.clone(),
                cwd: self.cwd.clone(),
                model_allowed: true,
                tools_allowed: true,
            });
        chain.bind_operations(
            operations
                .operations
                .as_ref()
                .expect("bound operations")
                .clone(),
        );
        let model = self.model_service.as_ref().map(|service| {
            Arc::new(
                BoundModel::new(
                    service.clone(),
                    model_binding,
                    self.runtime_config.model_timeout_ms,
                )
                .with_hooks(hooks.clone(), attribution, self.cwd.clone())
                .with_tool_authority(tools),
            ) as Arc<dyn Model>
        });
        let mut execution = ExecutionContext::new(
            scope,
            self.runtime_config.model_timeout_ms,
            model,
            selected_tools,
            policy,
            approval,
        )
        .with_hooks(hooks)
        .with_operations(operations);
        execution.permission_grants = permission_grants;
        Ok(execution)
    }

    pub(crate) fn tools_for_model(&self, model: Option<&ModelRef>) -> Result<ToolRegistry> {
        let mut tools = ToolRegistry::new();
        for (_source, spec) in self.tools.entries() {
            if !matches!(
                spec.surface,
                crate::domain::ToolSurface::ProviderHosted { .. }
            ) {
                tools.register_from(&self.tools, &spec.name)?;
            }
        }
        if let Some(model) = model {
            let service = self
                .model_service
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("no model is configured"))?;
            crate::core::register_provider_hosted_tools(
                &mut tools,
                service.id().as_ref(),
                service.provider_hosted_tools(model)?,
            )?;
        }
        if let Some(names) = &self.tool_allowlist {
            tools = filter_tools(&tools, names)?;
        }
        Ok(tools)
    }

    /// Creates a standalone invocation without manufacturing conversation ids.
    pub fn workflow_execution_context(
        &self,
        scope: crate::contracts::ExecutionScope,
        approval: Arc<dyn crate::contracts::ApprovalTransport>,
        permission_mode: crate::domain::PermissionMode,
    ) -> Result<crate::contracts::WorkflowInvocationContext> {
        let binding = ModelExecutionBinding::detached(scope);
        let context = self.bind_context(&binding);
        let execution = self.execution_context(binding, approval, permission_mode)?;
        Ok(crate::contracts::WorkflowInvocationContext::Execution(
            crate::contracts::WorkflowExecutionContext {
                execution,
                context,
                model_ref: self.model_config.as_ref().map(|cfg| cfg.model_ref()),
                reasoning: self
                    .model_config
                    .as_ref()
                    .map(|cfg| cfg.reasoning.clone())
                    .unwrap_or_default(),
                instructions: self.instructions.clone(),
                context_timeout_ms: self.runtime_config.context_timeout_ms,
                permission_mode,
                intent: None,
            },
        ))
    }

    /// Wraps an already-bound generic execution in the chat/application
    /// dependencies required by the selected `Workflow`.
    ///
    /// Turn attribution, recorders, policy mode, model and context bindings are
    /// resolved before this factory is called; this method must not create a second
    /// `ExecutionScope` or re-read mutable runtime state.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn agent_workflow_context(
        &self,
        execution: ExecutionContext,
        context: Arc<dyn ContextBuilder>,
        session_id: SessionId,
        thread_id: ThreadId,
        turn_id: TurnId,
        model_ref: Option<ModelRef>,
        reasoning: ReasoningConfig,
        events: Arc<EventEmitter>,
        user_input: Arc<dyn UserInputTransport>,
    ) -> AgentWorkflowContext {
        let preparation = Arc::new(super::context_preparation::RuntimeContextPreparation::new(
            execution.clone(),
            crate::contracts::ExecutionAttribution::for_turn(
                execution.scope.execution_id,
                session_id,
                thread_id,
                turn_id,
            ),
            self.cwd.clone(),
            None,
        ));
        AgentWorkflowContext::new(
            execution,
            session_id,
            thread_id,
            turn_id,
            model_ref,
            reasoning,
            self.runtime_config.context_timeout_ms,
            events,
            context,
            user_input,
            preparation,
            self.agent_control.clone(),
        )
        .with_instructions(self.instructions.clone())
    }

    #[cfg(test)]
    pub(crate) fn replace_model_for_test(&mut self, model: Arc<dyn Model>) {
        self.model_service = Some(Arc::new(ModelService::new(model)));
    }
}

fn filter_tools(
    tools: &ToolRegistry,
    names: &std::collections::BTreeSet<String>,
) -> Result<ToolRegistry> {
    let mut selected = ToolRegistry::new();
    for (_, spec) in tools.entries() {
        if names.contains(&spec.name) {
            selected.register_from(tools, &spec.name)?;
        }
    }
    Ok(selected)
}

use super::AgentRuntime;
use proteus_contracts::app_protocol::addons::{
    AppAddonsUpdate, AppAgentPluginState, AppMcpServerState, AppSkillCatalog,
};

impl AgentRuntime {
    pub(crate) async fn addon_catalogs(
        &self,
    ) -> (
        AppAddonsUpdate,
        Vec<AppSkillCatalog>,
        Vec<AppMcpServerState>,
        Vec<AppAgentPluginState>,
    ) {
        let snapshot = self.snapshot().await;
        let mut catalogs = Vec::new();
        for (id, provider) in &snapshot.registry.context_providers {
            let result = provider.skill_catalog(&snapshot.registry.cwd).await;
            catalogs.push(match result {
                Ok(catalog) => AppSkillCatalog {
                    provider: id.clone(),
                    catalog,
                    error: None,
                },
                Err(error) => AppSkillCatalog {
                    provider: id.clone(),
                    catalog: None,
                    error: Some(format!("{error:#}")),
                },
            });
        }
        let config = snapshot.assembly_plan.config();
        (
            AppAddonsUpdate {
                addons: config.addons.clone(),
                mcp_servers: config.tools.mcp_servers.clone(),
            },
            catalogs,
            snapshot.registry.mcp_servers.clone(),
            snapshot.registry.plugins.clone(),
        )
    }
}

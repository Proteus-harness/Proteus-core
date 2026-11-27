use super::AppServerHandle;
use anyhow::Result;
use proteus_contracts::app_protocol::{addons::*, http::SetConfigBuilderRequest};

impl AppServerHandle {
    pub async fn addons_snapshot(&self) -> AppAddonsSnapshot {
        let (settings, catalogs, mcp_servers, plugins) = self.runtime.addon_catalogs().await;
        AppAddonsSnapshot {
            reload_error: self.profile_error.lock().await.clone(),
            writable: self.config_path.is_some(),
            settings,
            catalogs,
            mcp_servers,
            plugins,
        }
    }

    pub async fn set_addons(&self, update: AppAddonsUpdate) -> Result<AppAddonsSnapshot> {
        self.set_profile_config(SetConfigBuilderRequest {
            addon_settings: Some(update),
            ..Default::default()
        })
        .await?;
        Ok(self.addons_snapshot().await)
    }
}

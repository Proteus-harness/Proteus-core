use super::AppServerHandle;

impl AppServerHandle {
    /// Discovery only; independent of chat turns, transcript and UI extensions.
    pub async fn model_quota(
        &self,
    ) -> anyhow::Result<Option<crate::contracts::ModelQuotaSnapshot>> {
        let quota = self.runtime.model_quota().await;
        self.rearm_idle_worker_retirement().await;
        quota
    }
}

use super::*;
use crate::{core::session_journal::load_validated_usage, domain::SessionUsageSnapshot};

impl SessionStore {
    /// Живой writer отдаёт готовую проекцию; cold read валидирует журнал без
    /// захвата write ownership, исправления хвоста или изменения файлов.
    pub async fn usage_snapshot(&self) -> Result<SessionUsageSnapshot> {
        let writer = self.writer.lock().await;
        if let Some(snapshot) = writer.usage_snapshot(self.session_id) {
            return Ok(snapshot);
        }
        #[cfg(unix)]
        if let Some(cached) = self.cached_read_metadata() {
            return Ok(cached.usage);
        }
        let store = self.clone();
        let snapshot = tokio::task::spawn_blocking(move || {
            #[cfg(unix)]
            if let Some(usage) = store.fresh_read_usage() {
                return Ok(usage);
            }
            load_validated_usage(&store.session_dir, store.session_id)
        })
        .await?;
        drop(writer);
        snapshot
    }
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;

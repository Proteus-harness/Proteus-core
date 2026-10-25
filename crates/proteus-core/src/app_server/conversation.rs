use anyhow::Result;

use super::AppServerHandle;
use crate::contracts::ConversationView;

impl AppServerHandle {
    pub async fn conversation_view(&self) -> Result<ConversationView> {
        Ok(ConversationView::new(
            self.session_id(),
            env!("CARGO_PKG_VERSION"),
            self.runtime.conversation_snapshot().await?,
        ))
    }
}

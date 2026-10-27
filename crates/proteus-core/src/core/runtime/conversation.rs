//! Root-owned read capability. No session lookup or cross-thread access.
use crate::{
    contracts::{ConversationReader, ConversationSnapshot},
    model_standard::CanonicalMessage,
};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

impl super::AgentRuntime {
    /// The same read capability used by conversation-bound module operations.
    /// This does not start/resume a turn or mutate durable history.
    pub async fn conversation_snapshot(&self) -> Result<ConversationSnapshot> {
        TurnConversation {
            history: self.session.history.clone(),
            resident: self.session.history_resident.clone(),
            revision: self.session.history_revision.clone(),
            store: self.session.session_store.clone(),
            context: self.session.model_context.clone(),
        }
        .read()
        .await
    }
}

pub(super) struct TurnConversation {
    pub history: Arc<Mutex<Vec<CanonicalMessage>>>,
    pub resident: Arc<std::sync::atomic::AtomicBool>,
    pub revision: Arc<std::sync::atomic::AtomicU64>,
    pub store: Option<crate::core::SessionStore>,
    pub context: Arc<Mutex<crate::core::model_context::ModelContextState>>,
}

#[async_trait]
impl ConversationReader for TurnConversation {
    async fn read(&self) -> Result<ConversationSnapshot> {
        // Use checkpoint lock order: history before context.
        let messages = self.history.lock().await;
        if !self.resident.load(std::sync::atomic::Ordering::SeqCst) {
            let store = self
                .store
                .clone()
                .ok_or_else(|| anyhow::anyhow!("nonresident history has no session store"))?;
            return tokio::task::spawn_blocking(move || store.conversation_snapshot()).await?;
        }
        let context = self.context.lock().await;
        Ok(ConversationSnapshot {
            messages: messages.clone(),
            revision: match &self.store {
                Some(store) => store.history_revision().await?,
                None => self.revision.load(std::sync::atomic::Ordering::SeqCst),
            },
            model_context: context.snapshot(),
        })
    }
}

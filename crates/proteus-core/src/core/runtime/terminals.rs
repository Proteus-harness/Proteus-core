//! Session-local live terminal capabilities. Input never passes through execution recording.
use crate::contracts::{
    ToolTerminalAction, ToolTerminalController, ToolTerminalDescriptor, ToolTerminalHost,
    ToolTerminalRef, ToolTerminalResponse,
};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Default)]
pub(super) struct TerminalRegistry {
    entries: Mutex<VecDeque<(Uuid, Arc<dyn ToolTerminalController>)>>,
    admission: Mutex<()>,
}
impl TerminalRegistry {
    pub async fn control(
        &self,
        id: Uuid,
        action: ToolTerminalAction,
    ) -> Result<ToolTerminalResponse> {
        validate(&action)?;
        let controller = self
            .entries
            .lock()
            .await
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, controller)| controller.clone())
            .ok_or_else(|| anyhow::anyhow!("terminal is unavailable in this session"))?;
        controller.control(action).await
    }
}

#[async_trait]
impl ToolTerminalHost for TerminalRegistry {
    async fn publish(
        &self,
        descriptor: ToolTerminalDescriptor,
        controller: Arc<dyn ToolTerminalController>,
    ) -> Result<ToolTerminalRef> {
        ensure!(
            !descriptor.handle.is_empty() && descriptor.handle.len() <= 256,
            "invalid terminal handle"
        );
        ensure!(descriptor.title.len() <= 512, "terminal title too long");
        validate(&ToolTerminalAction::Resize {
            columns: descriptor.columns,
            rows: descriptor.rows,
        })?;
        let terminal = ToolTerminalRef {
            id: Uuid::new_v4(),
            title: descriptor.title,
            tty: descriptor.tty,
        };
        let _admission = self.admission.lock().await;
        let candidates = {
            let entries = self.entries.lock().await;
            if entries.len() >= 32 {
                entries.iter().cloned().collect::<Vec<_>>()
            } else {
                Vec::new()
            }
        };
        // Never evict a live process just because newer commands completed.
        // Probes run outside the registry lock and never consume module output.
        for (id, candidate) in candidates {
            let response = candidate
                .control(ToolTerminalAction::Read {
                    cursor: None,
                    max_bytes: 1,
                    wait_ms: 0,
                })
                .await;
            let removable = match response {
                Ok(ToolTerminalResponse::Read(snapshot)) => snapshot.finished,
                Ok(ToolTerminalResponse::Unavailable) => true,
                Err(error) => error.downcast_ref::<proteus_module_protocol::v3::ComponentBrokerError>()
                    .is_some_and(|error| error.kind == proteus_module_protocol::v3::ComponentBrokerErrorKind::RuntimeUnavailable),
                _ => false,
            };
            if removable {
                self.entries.lock().await.retain(|(key, _)| *key != id);
                break;
            }
        }
        let mut entries = self.entries.lock().await;
        ensure!(
            entries.len() < 32,
            "too many live terminals in this session"
        );
        entries.push_back((terminal.id, controller));
        Ok(terminal)
    }
}

fn validate(action: &ToolTerminalAction) -> Result<()> {
    use base64::Engine;
    match action {
        ToolTerminalAction::Read {
            max_bytes, wait_ms, ..
        } => {
            ensure!(
                *max_bytes > 0 && *max_bytes <= 65_536,
                "invalid terminal read limit"
            );
            ensure!(*wait_ms <= 1_000, "invalid terminal wait limit");
        }
        ToolTerminalAction::Input { data_base64 } => {
            ensure!(data_base64.len() <= 87_384, "terminal input exceeds limit");
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data_base64)
                .map_err(|_| anyhow::anyhow!("invalid terminal input encoding"))?;
            ensure!(bytes.len() <= 65_536, "terminal input exceeds limit");
        }
        ToolTerminalAction::Resize { columns, rows } => ensure!(
            (2..=1000).contains(columns) && (1..=500).contains(rows),
            "invalid terminal dimensions"
        ),
        ToolTerminalAction::Stop => {}
    }
    Ok(())
}

impl super::AgentRuntime {
    pub async fn terminal_control(
        &self,
        id: Uuid,
        action: ToolTerminalAction,
    ) -> Result<ToolTerminalResponse> {
        self.services.terminals.control(id, action).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Probe(Arc<AtomicUsize>);
    #[async_trait]
    impl ToolTerminalController for Probe {
        async fn control(&self, _: ToolTerminalAction) -> Result<ToolTerminalResponse> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolTerminalResponse::Accepted)
        }
    }
    fn descriptor() -> ToolTerminalDescriptor {
        ToolTerminalDescriptor {
            handle: "private".into(),
            title: "command".into(),
            tty: true,
            columns: 80,
            rows: 24,
        }
    }
    #[tokio::test]
    async fn session_isolation_limits_and_view_drop_do_not_stop_process() -> Result<()> {
        let registry = TerminalRegistry::default();
        let foreign = TerminalRegistry::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let terminal = registry
            .publish(descriptor(), Arc::new(Probe(calls.clone())))
            .await?;
        assert!(
            foreign
                .control(terminal.id, ToolTerminalAction::Stop)
                .await
                .is_err()
        );
        assert!(
            registry
                .control(
                    terminal.id,
                    ToolTerminalAction::Input {
                        data_base64: "!".into()
                    }
                )
                .await
                .is_err()
        );
        assert!(
            registry
                .control(
                    terminal.id,
                    ToolTerminalAction::Read {
                        cursor: None,
                        max_bytes: 65_537,
                        wait_ms: 0
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        registry
            .control(
                terminal.id,
                ToolTerminalAction::Input {
                    data_base64: "c2VjcmV0".into(),
                },
            )
            .await?;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Dropping view data performs no controller action. Only explicit stop does.
        let id = terminal.id;
        drop(terminal);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        registry.control(id, ToolTerminalAction::Stop).await?;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        for _ in 0..31 {
            registry
                .publish(descriptor(), Arc::new(Probe(calls.clone())))
                .await?;
        }
        assert_eq!(registry.entries.lock().await.len(), 32);
        assert!(
            registry
                .publish(descriptor(), Arc::new(Probe(calls.clone())))
                .await
                .is_err()
        );
        // The original capability remains usable despite the full registry.
        registry.control(id, ToolTerminalAction::Stop).await?;
        Ok(())
    }
}

#[cfg(test)]
mod eviction_tests {
    use super::*;
    struct Record(bool);
    #[async_trait]
    impl ToolTerminalController for Record {
        async fn control(&self, _: ToolTerminalAction) -> Result<ToolTerminalResponse> {
            if self.0 {
                Ok(ToolTerminalResponse::Unavailable)
            } else {
                Ok(ToolTerminalResponse::Accepted)
            }
        }
    }
    #[tokio::test]
    async fn expired_module_handle_frees_capacity_without_revoking_live_record() -> Result<()> {
        let registry = TerminalRegistry::default();
        let descriptor = || ToolTerminalDescriptor {
            handle: "private".into(),
            title: "test".into(),
            tty: true,
            columns: 80,
            rows: 24,
        };
        let live = registry
            .publish(descriptor(), Arc::new(Record(false)))
            .await?;
        let expired = registry
            .publish(descriptor(), Arc::new(Record(true)))
            .await?;
        for _ in 0..30 {
            registry
                .publish(descriptor(), Arc::new(Record(false)))
                .await?;
        }
        registry
            .publish(descriptor(), Arc::new(Record(false)))
            .await?;
        assert_eq!(registry.entries.lock().await.len(), 32);
        assert!(
            registry
                .control(expired.id, ToolTerminalAction::Stop)
                .await
                .is_err()
        );
        assert!(matches!(
            registry.control(live.id, ToolTerminalAction::Stop).await?,
            ToolTerminalResponse::Accepted
        ));
        Ok(())
    }
}

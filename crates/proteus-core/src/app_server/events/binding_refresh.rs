//! A background child can bind after the parent turn has settled. Refresh its
//! saved tool link once per burst, without polling or retaining another runtime.
use super::AppEventPublisher;
use crate::{app_server::AppServerEvent, core::SessionStore};

#[derive(Default)]
pub(super) struct State {
    running: bool,
    pending: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proteus_contracts::app_protocol::{AppExecutionState, AppRun, AppRunStatus};

    #[tokio::test]
    async fn binding_after_settlement_read_is_retained_until_idle_publication() {
        let (events, _) = super::super::test_event_channel(8);
        let _ = events.send(AppServerEvent::ExecutionUpdated {
            execution: AppExecutionState {
                active: Some(AppRun {
                    run_id: "settling-parent".into(),
                    options: Default::default(),
                    status: AppRunStatus::Running,
                    error: None,
                }),
                last: None,
            },
        });
        // The parent has read its journal prefix, but has not published idle.
        // A child binding arriving now must survive the stale prefix's install.
        for _ in 0..10 {
            events.refresh_idle_child_binding();
        }
        {
            let state = events.0.binding_refresh.lock().unwrap();
            assert!(state.pending);
            assert!(
                !state.running,
                "an active parent must not start a projection"
            );
        }
        events.finish_progress(Ok(Vec::new().into()));
        assert!(events.0.binding_refresh.lock().unwrap().pending);
        let _ = events.send(AppServerEvent::ExecutionUpdated {
            execution: Default::default(),
        });
        assert!(events.0.binding_refresh.lock().unwrap().running);
        // This publisher has no runtime/store. The scheduled drain still must
        // finish and consume the request; no extra runtime is manufactured.
        tokio::time::timeout(std::time::Duration::from_millis(200), async {
            loop {
                if !events.0.binding_refresh.lock().unwrap().running {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!events.0.binding_refresh.lock().unwrap().pending);
    }
}

impl AppEventPublisher {
    pub(super) fn refresh_idle_child_binding(&self) {
        self.0
            .binding_refresh
            .lock()
            .expect("binding refresh lock")
            .pending = true;
        self.drain_child_bindings();
    }

    pub(super) fn drain_child_bindings(&self) {
        {
            let view = self.0.view.lock().expect("session view lock");
            // The settlement projection can precede a late append. Retain the
            // request even while active and drain it on the idle publication.
            if view.run_is_active() {
                return;
            }
        }
        let mut refresh = self.0.binding_refresh.lock().expect("binding refresh lock");
        if !refresh.pending || refresh.running {
            return;
        }
        refresh.running = true;
        drop(refresh);
        let events = self.clone();
        tokio::spawn(async move {
            loop {
                let source = {
                    let view = events.0.view.lock().expect("session view lock");
                    let mut refresh = events
                        .0
                        .binding_refresh
                        .lock()
                        .expect("binding refresh lock");
                    if view.run_is_active() || !refresh.pending {
                        refresh.running = false;
                        return;
                    }
                    refresh.pending = false;
                    let dir = view.session_dir();
                    view.history.as_ref().ok().cloned().zip(dir)
                };
                if let Some((baseline, dir)) = source {
                    let result = tokio::task::spawn_blocking(move || {
                        let store = SessionStore::open(dir)?;
                        crate::app_server::read_model::project_completed_transcript(&store)
                    })
                    .await;
                    let result = match result {
                        Ok(result) => result,
                        Err(error) => Err(anyhow::anyhow!(
                            "child binding projection task failed: {error}"
                        )),
                    };
                    match result {
                        Ok(history) => {
                            // A newer settlement/clear wins over this refresh.
                            match events.refresh_idle_history(&baseline, history) {
                                Ok(true) => {}
                                Ok(false) => {
                                    // A new admission may have overtaken this
                                    // blocking read. Its idle edge must retry.
                                    events
                                        .0
                                        .binding_refresh
                                        .lock()
                                        .expect("binding refresh lock")
                                        .pending = true;
                                }
                                Err(error) => {
                                    let _ = events.send(AppServerEvent::Error {
                                        message: format!(
                                            "Не удалось обновить ссылку на чат субагента: {error:#}"
                                        ),
                                    });
                                }
                            }
                        }
                        Err(error) => {
                            let message =
                                format!("Не удалось обновить ссылку на чат субагента: {error:#}");
                            eprintln!("warning: {message}");
                            let _ = events.send(AppServerEvent::Error { message });
                        }
                    }
                }
                let mut refresh = events
                    .0
                    .binding_refresh
                    .lock()
                    .expect("binding refresh lock");
                if !refresh.pending {
                    refresh.running = false;
                    return;
                }
            }
        });
    }
}

//! Bootstrap/rotation, exact model selection, then one child Send under one budget.
use super::super::mailbox::ChildMailbox;
use super::{
    RunnerInner,
    pool::PooledChild,
    turn::{ChildEventForwarder, TurnEnd, TurnTracker, drive_turn, finish_interrupted},
};
use crate::{
    contracts::{AgentControlRequest, AgentLifecycleStatus, AgentProfile},
    core::ModelConfig,
    domain::new_call_id,
};
use anyhow::Result;
use proteus_contracts::app_protocol::StdioRequest;
use std::time::Duration;
use tokio::time::{Instant, timeout_at};

impl RunnerInner {
    /// Turn на арендованном процессе: durable session bootstrap/rotation, `Send`,
    /// затем drive до терминального Response с уважением role-таймаута.
    pub(super) async fn run_leased_turn(
        &self,
        profile: &AgentProfile,
        request: &AgentControlRequest,
        forwarder: &ChildEventForwarder<'_>,
        tracker: &mut TurnTracker,
        leased: &mut PooledChild,
        mailbox: &ChildMailbox,
        is_resume: bool,
        model: Option<&ModelConfig>,
    ) -> Result<TurnEnd> {
        let deadline = self
            .roles
            .get(&profile.name)
            .and_then(|role| role.config.timeout_ms)
            .map(|millis| Instant::now() + Duration::from_millis(millis));
        leased.child.set_write_budget(
            deadline,
            Some(forwarder.ctx.execution.scope.cancellation.clone()),
        );
        let send_id = new_call_id();
        let body = async {
            leased.child.drain_stale_outputs()?;
            if !is_resume && leased.used {
                leased.child.new_session().await?
            } else {
                leased.child.ensure_session().await?
            };
            if !is_resume && let Some(model) = model {
                leased.child.set_model_configuration(model).await?;
            }
            leased
                .child
                .send(&StdioRequest::Send {
                    images: Vec::new(),
                    options: Default::default(),
                    id: Some(send_id.clone()),
                    text: request.prompt.clone(),
                })
                .await?;
            tracker.active_send_id = Some(send_id.clone());
            drive_turn(&mut leased.child, forwarder, &send_id, tracker, mailbox).await
        };
        let result = match deadline {
            Some(deadline) => timeout_at(deadline, body).await.unwrap_or_else(|_| {
                let status = if forwarder.ctx.execution.scope.cancellation.is_cancelled() {
                    AgentLifecycleStatus::Cancelled
                } else {
                    AgentLifecycleStatus::TimedOut
                };
                Ok(TurnEnd::Interrupted(status))
            }),
            None => body.await,
        };
        let interrupted = if forwarder.ctx.execution.scope.cancellation.is_cancelled() {
            Some(AgentLifecycleStatus::Cancelled)
        } else if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            Some(AgentLifecycleStatus::TimedOut)
        } else {
            None
        };
        let result = match result {
            Ok(TurnEnd::Interrupted(status)) => Ok(finish_interrupted(
                &mut leased.child,
                forwarder,
                tracker,
                self.cancel_grace,
                status,
            )
            .await),
            Err(error) => {
                // A protocol/setup/write error remains fatal even if a cancel
                // or deadline became visible at the same time.
                leased.child.kill().await;
                match interrupted {
                    Some(status) => Ok(TurnEnd::Interrupted(status)),
                    None => Err(error),
                }
            }
            completed => completed,
        };
        leased.child.set_write_budget(None, None);
        result
    }
}

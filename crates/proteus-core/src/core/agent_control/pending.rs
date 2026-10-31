//! Реестр запущенных (`spawn`) process peers: `JoinHandle` + cancel-токен на
//! spawn_id.
//!
//! Жизненный цикл записи: `reserve` (жёсткий cap по `max_parallel`) →
//! `attach` (detached monitor забирает JoinHandle) → кешированный terminal
//! outcome. Waiter никогда не владеет JoinHandle, поэтому его отмена не теряет
//! результат; завершённая запись вытесняется при следующем `reserve`, когда
//! реестр упирается в cap.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{Result, anyhow, bail};
use tokio::{sync::Notify, task::JoinHandle};

use crate::contracts::{AgentAddress, AgentControlMessage, AgentControlResult, CancellationToken};

use super::mailbox::ChildMailbox;

#[derive(Default)]
pub(super) struct PendingChildren {
    entries: HashMap<String, PendingChild>,
    next_seq: u64,
}

pub(super) struct PendingChild {
    seq: u64,
    cancel: CancellationToken,
    mailbox: Arc<ChildMailbox>,
    agent_target: Option<AgentAddress>,
    outcome: Arc<PendingOutcome>,
    evictable_when_ready: bool,
}

#[derive(Default)]
pub(super) struct PendingOutcome {
    state: Mutex<PendingOutcomeState>,
    notify: Notify,
}

#[derive(Default)]
struct PendingOutcomeState {
    result: Option<std::result::Result<AgentControlResult, String>>,
    consumed: bool,
}

impl PendingChildren {
    /// Резервирует слот под ребёнка. Если реестр упёрся в `max_parallel`,
    /// сначала вытесняются самые старые завершённые-но-не-затребованные
    /// записи; активные дети не вытесняются никогда.
    pub(super) fn reserve(
        &mut self,
        spawn_id: &str,
        cancel: CancellationToken,
        mailbox: Arc<ChildMailbox>,
        agent_target: Option<AgentAddress>,
        max_parallel: usize,
        evictable_when_ready: bool,
    ) -> Result<()> {
        if self.entries.contains_key(spawn_id) {
            bail!("duplicate subagent spawn_id: {spawn_id}");
        }
        while self.entries.len() >= max_parallel {
            let finished_oldest = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.evictable_when_ready && entry.outcome.is_ready())
                .min_by_key(|(_, entry)| entry.seq)
                .map(|(id, _)| id.clone());
            match finished_oldest {
                Some(id) => {
                    self.entries.remove(&id);
                }
                None => bail!(
                    "too many concurrent subagent children (max_parallel {max_parallel}); \
                     wait for spawned tasks before starting new ones"
                ),
            }
        }
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.entries.insert(
            spawn_id.to_owned(),
            PendingChild {
                seq,
                cancel,
                mailbox,
                agent_target,
                outcome: Arc::new(PendingOutcome::default()),
                evictable_when_ready,
            },
        );
        Ok(())
    }

    /// Прикрепляет JoinHandle к зарезервированному слоту.
    pub(super) fn attach(&mut self, spawn_id: &str, join: JoinHandle<Result<AgentControlResult>>) {
        if let Some(entry) = self.entries.get_mut(spawn_id) {
            let outcome = entry.outcome.clone();
            tokio::spawn(async move {
                let result = match join.await {
                    Ok(Ok(result)) => Ok(result),
                    Ok(Err(error)) => Err(format!("{error:#}")),
                    Err(error) => Err(format!("subagent child task failed: {error}")),
                };
                outcome.complete(result);
            });
        } else {
            // Слот сняли между reserve и attach (не должен случаться:
            // оба вызова идут подряд в spawn) — не оставляем таску без
            // индивидуальной отмены незаметно.
            join.abort();
        }
    }

    /// Снимает резервацию (ошибка между reserve и attach).
    pub(super) fn release(&mut self, spawn_id: &str) {
        self.entries.remove(spawn_id);
    }

    /// Возвращает разделяемый terminal outcome для cancellation-safe wait.
    pub(super) fn outcome(&self, spawn_id: &str) -> Result<Arc<PendingOutcome>> {
        let entry = self.entries.get(spawn_id).ok_or_else(|| {
            anyhow!("unknown subagent spawn_id (never spawned, already waited, or evicted)")
        })?;
        Ok(entry.outcome.clone())
    }

    /// Claims the outcome already acquired by a waiter, even if cap pressure
    /// evicted its ready index entry. Cancellation before this point does not
    /// consume it; concurrent waiters still cannot both return the result.
    pub(super) fn consume(&mut self, spawn_id: &str, outcome: &Arc<PendingOutcome>) -> Result<()> {
        outcome.consume()?;
        if self
            .entries
            .get(spawn_id)
            .is_some_and(|entry| Arc::ptr_eq(&entry.outcome, outcome))
        {
            self.entries.remove(spawn_id);
        }
        Ok(())
    }

    /// Process peers cannot append an undelivered local envelope to the
    /// child's canonical history after its stdio turn is cancelled. Close the
    /// queue before signaling cancel so a later send is rejected explicitly.
    pub(super) fn begin_cancel_and_close_mailbox(
        &mut self,
        spawn_id: &str,
    ) -> Result<Arc<ChildMailbox>> {
        let entry = self.entries.get(spawn_id).ok_or_else(|| {
            anyhow!("unknown subagent spawn_id (never spawned, already waited, or evicted)")
        })?;
        entry.mailbox.close_and_discard()?;
        entry.cancel.cancel();
        Ok(entry.mailbox.clone())
    }

    pub(super) fn send(&self, spawn_id: &str, message: AgentControlMessage) -> Result<usize> {
        let entry = self.entries.get(spawn_id).ok_or_else(|| {
            anyhow!("unknown subagent spawn_id (never spawned, already waited, or evicted)")
        })?;
        message.validate()?;
        let target = entry.agent_target.as_ref().ok_or_else(|| {
            anyhow!("subagent spawn is not owned by the collaboration control plane")
        })?;
        if &message.target != target {
            bail!(
                "agent-control target {} does not match handle target {}",
                message.target,
                target
            );
        }
        entry.mailbox.enqueue(message)
    }
}

impl PendingOutcome {
    fn is_ready(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .result
            .is_some()
    }

    fn complete(&self, result: std::result::Result<AgentControlResult, String>) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .result = Some(result);
        self.notify.notify_waiters();
    }

    fn consume(&self) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.result.is_none() {
            bail!("subagent result is not ready to consume");
        }
        if state.consumed {
            bail!("subagent result was already consumed");
        }
        state.consumed = true;
        Ok(())
    }

    pub(super) async fn wait(&self) -> Result<AgentControlResult> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(result) = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .result
                .clone()
            {
                return result.map_err(anyhow::Error::msg);
            }
            notified.as_mut().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::AgentLifecycleStatus;
    fn dummy_join() -> JoinHandle<Result<AgentControlResult>> {
        tokio::spawn(async { Ok(AgentControlResult::new("", AgentLifecycleStatus::Completed)) })
    }

    fn mailbox() -> Arc<ChildMailbox> {
        Arc::new(ChildMailbox::default())
    }

    #[tokio::test]
    async fn reserve_attach_take_round_trip() {
        let mut pending = PendingChildren::default();
        pending
            .reserve("a", CancellationToken::new(), mailbox(), None, 4, true)
            .expect("reserve");
        pending.attach("a", dummy_join());
        let outcome = pending.outcome("a").expect("outcome");
        let result = outcome.wait().await.expect("result");
        assert_eq!(result.status, AgentLifecycleStatus::Completed);
        let repeated = outcome.wait().await.expect("repeatable result");
        assert_eq!(repeated.status, AgentLifecycleStatus::Completed);
    }

    #[tokio::test]
    async fn reserve_respects_max_parallel_for_active_children() {
        let mut pending = PendingChildren::default();
        pending
            .reserve("a", CancellationToken::new(), mailbox(), None, 1, true)
            .expect("reserve");
        // Слот занят активной (не завершённой) резервацией: cap жёсткий.
        let error = pending
            .reserve("b", CancellationToken::new(), mailbox(), None, 1, true)
            .expect_err("cap");
        assert!(error.to_string().contains("max_parallel"));
    }

    #[tokio::test]
    async fn reserve_evicts_finished_unclaimed_entries() {
        let mut pending = PendingChildren::default();
        pending
            .reserve("a", CancellationToken::new(), mailbox(), None, 1, true)
            .expect("reserve");
        pending.attach("a", dummy_join());
        let outcome = pending.outcome("a").expect("outcome");
        while !outcome.is_ready() {
            tokio::task::yield_now().await;
        }
        pending
            .reserve("b", CancellationToken::new(), mailbox(), None, 1, true)
            .expect("evicts finished entry");
        assert!(pending.outcome("a").is_err(), "запись вытеснена");
    }

    #[tokio::test]
    async fn reserve_does_not_evict_control_owned_completion_before_monitor_waits() {
        let mut pending = PendingChildren::default();
        pending
            .reserve("a", CancellationToken::new(), mailbox(), None, 1, false)
            .expect("reserve retained child");
        pending.attach("a", dummy_join());
        let outcome = pending.outcome("a").expect("outcome");
        while !outcome.is_ready() {
            tokio::task::yield_now().await;
        }

        let error = pending
            .reserve("b", CancellationToken::new(), mailbox(), None, 1, true)
            .expect_err("control-owned result must remain addressable");
        assert!(error.to_string().contains("max_parallel"));
        assert!(pending.outcome("a").is_ok());
    }

    #[tokio::test]
    async fn cancel_cancels_only_target_token() {
        let mut pending = PendingChildren::default();
        let first = CancellationToken::new();
        let second = CancellationToken::new();
        pending
            .reserve("a", first.clone(), mailbox(), None, 4, true)
            .expect("reserve a");
        pending
            .reserve("b", second.clone(), mailbox(), None, 4, true)
            .expect("reserve b");
        pending.begin_cancel_and_close_mailbox("a").expect("cancel");
        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());
        assert!(pending.begin_cancel_and_close_mailbox("missing").is_err());
    }

    #[tokio::test]
    async fn cancel_remains_available_while_wait_is_active() {
        let mut pending = PendingChildren::default();
        let token = CancellationToken::new();
        pending
            .reserve("a", token.clone(), mailbox(), None, 1, true)
            .expect("reserve");
        pending.attach("a", dummy_join());
        let outcome = pending.outcome("a").expect("outcome");
        pending
            .begin_cancel_and_close_mailbox("a")
            .expect("cancel during wait");
        assert!(token.is_cancelled());
        let _ = outcome.wait().await;
    }

    #[tokio::test]
    async fn active_waiter_claims_terminal_result_after_ready_entry_is_evicted() {
        let mut pending = PendingChildren::default();
        pending
            .reserve("a", CancellationToken::new(), mailbox(), None, 1, true)
            .unwrap();
        let outcome = pending.outcome("a").unwrap();
        outcome.complete(Ok(AgentControlResult::new(
            "finished",
            AgentLifecycleStatus::Completed,
        )));

        // The monitor completed, but another admission runs before the waiter.
        pending
            .reserve("b", CancellationToken::new(), mailbox(), None, 1, true)
            .unwrap();
        assert!(
            pending.outcome("a").is_err(),
            "the old handle is no longer discoverable"
        );
        let result = outcome.wait().await.unwrap();
        pending.consume("a", &outcome).unwrap();
        assert_eq!(result.summary, "finished");
        assert!(pending.outcome("b").is_ok());
        assert!(pending.consume("a", &outcome).is_err());
    }

    #[tokio::test]
    async fn concurrent_waiters_claim_a_terminal_error_only_once() {
        let mut pending = PendingChildren::default();
        pending
            .reserve("a", CancellationToken::new(), mailbox(), None, 1, true)
            .unwrap();
        let first = pending.outcome("a").unwrap();
        let second = pending.outcome("a").unwrap();
        assert!(pending.consume("a", &first).is_err(), "not terminal yet");
        first.complete(Err("child failed".to_owned()));
        assert_eq!(first.wait().await.unwrap_err().to_string(), "child failed");
        assert_eq!(second.wait().await.unwrap_err().to_string(), "child failed");
        pending.consume("a", &first).unwrap();
        assert!(pending.consume("a", &second).is_err());
    }

    #[tokio::test]
    async fn old_outcome_claim_does_not_remove_a_replacement_entry() {
        let mut pending = PendingChildren::default();
        pending
            .reserve("a", CancellationToken::new(), mailbox(), None, 1, true)
            .unwrap();
        let old = pending.outcome("a").unwrap();
        old.complete(Ok(AgentControlResult::new(
            "old",
            AgentLifecycleStatus::Completed,
        )));
        pending
            .reserve("b", CancellationToken::new(), mailbox(), None, 1, true)
            .unwrap();
        pending.release("b");
        pending
            .reserve("a", CancellationToken::new(), mailbox(), None, 1, true)
            .unwrap();
        let replacement = pending.outcome("a").unwrap();

        assert_eq!(old.wait().await.unwrap().summary, "old");
        pending.consume("a", &old).unwrap();
        assert!(Arc::ptr_eq(&pending.outcome("a").unwrap(), &replacement));
        assert!(!replacement.is_ready());
    }

    #[tokio::test]
    async fn dropped_wait_future_does_not_consume_later_terminal_result() {
        let mut pending = PendingChildren::default();
        pending
            .reserve("a", CancellationToken::new(), mailbox(), None, 1, true)
            .unwrap();
        let outcome = pending.outcome("a").unwrap();
        {
            let wait = outcome.wait();
            tokio::pin!(wait);
            tokio::select! {
                biased;
                result = &mut wait => panic!("result is not terminal yet: {result:?}"),
                _ = std::future::ready(()) => {}
            }
        }
        outcome.complete(Ok(AgentControlResult::new(
            "later",
            AgentLifecycleStatus::Completed,
        )));
        let later = pending.outcome("a").unwrap();
        assert_eq!(later.wait().await.unwrap().summary, "later");
        pending.consume("a", &later).unwrap();
        assert!(pending.outcome("a").is_err());
    }
}

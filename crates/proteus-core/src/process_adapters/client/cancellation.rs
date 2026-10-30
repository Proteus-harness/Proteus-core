use std::time::Duration;

use tokio::time::{Interval, MissedTickBehavior, interval};

use crate::contracts::CancellationToken;

/// Invocation-local wait policy; never owns cancellation or execution authority.
pub(super) enum CancellationSource<'a, F> {
    Token(&'a CancellationToken),
    Check { check: F, poll: Option<Interval> },
}

impl<F: Fn() -> bool> CancellationSource<'_, F> {
    pub(super) fn is_cancelled(&self) -> bool {
        match self {
            Self::Token(token) => token.is_cancelled(),
            Self::Check { check, .. } => check(),
        }
    }

    pub(super) fn begin_wait(&mut self) {
        if let Self::Check { poll, .. } = self {
            // Bool-only hosts have no wake source. Preserve their immediate
            // first check and fresh 10 ms polling interval in each phase.
            let mut interval = interval(Duration::from_millis(10));
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            *poll = Some(interval);
        }
    }

    pub(super) async fn observe(&mut self) -> bool {
        match self {
            Self::Token(token) => {
                token.cancelled().await;
                true
            }
            Self::Check { check, poll } => {
                poll.as_mut()
                    .expect("cancellation wait not started")
                    .tick()
                    .await;
                // Return after one tick so the outer select re-polls admission
                // or terminal even when a slow/stateful predicate returns false.
                check()
            }
        }
    }
}

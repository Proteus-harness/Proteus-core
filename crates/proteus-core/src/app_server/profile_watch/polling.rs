//! Production timing and a test-only, allocation-free-on-idle control seam.
use super::{ConfigInputs, Instant};
use std::time::Duration;

pub(super) struct Polling {
    interval: tokio::time::Interval,
    #[cfg(test)]
    requests: Option<tokio::sync::mpsc::UnboundedReceiver<Poll>>,
}

impl Polling {
    pub(super) fn new() -> Self {
        let mut interval = tokio::time::interval(Duration::from_millis(300));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Self {
            interval,
            #[cfg(test)]
            requests: None,
        }
    }

    pub(super) async fn next(&mut self) -> Option<Poll> {
        #[cfg(test)]
        if let Some(requests) = &mut self.requests {
            return requests.recv().await;
        }
        self.interval.tick().await;
        Some(Poll {
            now: Instant::now(),
            #[cfg(test)]
            force_current: false,
            #[cfg(test)]
            report: PollReport::default(),
            #[cfg(test)]
            completed: None,
        })
    }

    #[cfg(test)]
    pub(super) fn controlled() -> (Self, tokio::sync::mpsc::UnboundedSender<Poll>) {
        let (sender, requests) = tokio::sync::mpsc::unbounded_channel();
        let mut polling = Self::new();
        polling.requests = Some(requests);
        (polling, sender)
    }
}

pub(super) struct Poll {
    pub(super) now: Instant,
    #[cfg(test)]
    force_current: bool,
    #[cfg(test)]
    report: PollReport,
    #[cfg(test)]
    completed: Option<tokio::sync::oneshot::Sender<PollReport>>,
}

impl Poll {
    pub(super) fn inputs_current(&self, inputs: Option<&ConfigInputs>) -> bool {
        inputs.is_some_and(|inputs| {
            #[cfg(test)]
            if self.force_current {
                return true;
            }
            inputs.is_current()
        })
    }

    pub(super) fn record_scan(&mut self) {
        #[cfg(test)]
        {
            self.report.scanned = true;
        }
    }

    pub(super) fn record_refresh(&mut self) {
        #[cfg(test)]
        {
            self.report.refresh_attempted = true;
        }
    }

    #[cfg(test)]
    pub(super) fn controlled(
        now: Instant,
        completed: tokio::sync::oneshot::Sender<PollReport>,
    ) -> Self {
        Self {
            now,
            force_current: true,
            report: PollReport::default(),
            completed: Some(completed),
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PollReport {
    pub(super) scanned: bool,
    pub(super) refresh_attempted: bool,
}

#[cfg(test)]
impl Drop for Poll {
    fn drop(&mut self) {
        if let Some(completed) = self.completed.take() {
            let _ = completed.send(self.report);
        }
    }
}

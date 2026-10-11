//! Which activity changes of a session deserve the user's attention.
//!
//! The first status seen for a session is a baseline, not news, so opening
//! the client never reports what happened before it started.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use proteus_contracts::app_protocol::AppSessionActivityStatus as Status;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Attention {
    Finished,
    Approval,
    Input,
}

impl Attention {
    fn between(previous: Status, next: Status) -> Option<Self> {
        if previous == next {
            return None;
        }
        match next {
            Status::Idle => Some(Self::Finished),
            Status::WaitingApproval => Some(Self::Approval),
            Status::WaitingInput => Some(Self::Input),
            Status::Running => None,
        }
    }

    pub fn body(self) -> &'static str {
        match self {
            Self::Finished => "Агент закончил работу.",
            Self::Approval => "Агент ждёт подтверждения действия.",
            Self::Input => "Агент задал вопрос и ждёт ответа.",
        }
    }
}

#[derive(Default)]
pub struct Tracker {
    seen: HashMap<PathBuf, Status>,
}

impl Tracker {
    /// Records a status; returns the news when it changed since last time.
    pub fn observe(&mut self, dir: &Path, status: Status) -> Option<Attention> {
        let previous = self.seen.insert(dir.to_path_buf(), status)?;
        Attention::between(previous, status)
    }

    /// Takes a status from a full listing without reporting it.
    pub fn baseline(&mut self, dir: &Path, status: Status) {
        self.seen.entry(dir.to_path_buf()).or_insert(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_changes_after_the_baseline() {
        let mut tracker = Tracker::default();
        let dir = Path::new("/s");
        assert_eq!(tracker.observe(dir, Status::Running), None);
        assert_eq!(tracker.observe(dir, Status::Running), None);
        assert_eq!(
            tracker.observe(dir, Status::WaitingApproval),
            Some(Attention::Approval)
        );
        assert_eq!(tracker.observe(dir, Status::Running), None);
        assert_eq!(
            tracker.observe(dir, Status::Idle),
            Some(Attention::Finished)
        );
        tracker.baseline(dir, Status::Running);
        assert_eq!(tracker.observe(dir, Status::Idle), None);
    }
}

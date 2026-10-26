//! A weak registry may replace a writer only after its OS lease has dropped.
use std::sync::{Arc, Condvar, Mutex};

#[derive(Debug, Default)]
pub(crate) struct WriterRetirement {
    completed: Mutex<bool>,
    changed: Condvar,
}

impl WriterRetirement {
    pub(crate) fn completed(&self) -> bool {
        *self
            .completed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub(crate) fn wait(&self) {
        let mut completed = self
            .completed
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while !*completed {
            completed = self
                .changed
                .wait(completed)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

#[derive(Debug)]
pub(super) struct RetirementGuard(pub(super) Arc<WriterRetirement>);

impl Drop for RetirementGuard {
    fn drop(&mut self) {
        *self
            .0
            .completed
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        self.0.changed.notify_all();
    }
}

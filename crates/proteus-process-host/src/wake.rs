//! Coalesced notifications shared by transport workers and a protocol owner.
use std::{
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

/// Capture a revision before examining queues, then wait for it to change.
/// Notifications between examining a queue and waiting cannot be lost.
#[derive(Clone, Debug, Default)]
pub struct ProcessWake {
    inner: Arc<Signal>,
}

#[derive(Debug, Default)]
struct Signal {
    revision: Mutex<u64>,
    changed: Condvar,
}

impl ProcessWake {
    pub fn revision(&self) -> u64 {
        *self
            .inner
            .revision
            .lock()
            .expect("process wake lock poisoned")
    }

    pub fn notify(&self) {
        let mut revision = self
            .inner
            .revision
            .lock()
            .expect("process wake lock poisoned");
        *revision = revision.wrapping_add(1);
        self.inner.changed.notify_all();
    }

    pub fn wait(&self, observed: u64, timeout: Option<Duration>) {
        let revision = self
            .inner
            .revision
            .lock()
            .expect("process wake lock poisoned");
        match timeout {
            Some(timeout) => {
                let _guard = self
                    .inner
                    .changed
                    .wait_timeout_while(revision, timeout, |value| *value == observed)
                    .expect("process wake lock poisoned while waiting");
            }
            None => {
                let _guard = self
                    .inner
                    .changed
                    .wait_while(revision, |value| *value == observed)
                    .expect("process wake lock poisoned while waiting");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_before_wait_is_not_lost() {
        let wake = ProcessWake::default();
        let revision = wake.revision();
        wake.notify();
        // Unbounded: regressing to a bare Condvar wait would hang this waiter.
        let other = wake.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            other.wait(revision, None);
            tx.send(()).unwrap();
        });
        let result = rx.recv_timeout(Duration::from_secs(2));
        wake.notify();
        thread.join().unwrap();
        result.unwrap();
    }
}

use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{SendError, TryRecvError, TrySendError},
    },
};

use proteus_process_host::ProcessWake;

// Keep the common small allocation across bursts. Return the allocation of a
// deep queue when drained so unusual saturation does not fix its idle cost.
const REUSABLE_SLOTS: usize = 8;

struct Channel<T> {
    capacity: usize,
    available: Condvar,
    state: Mutex<State<T>>,
    wake: ProcessWake,
}

struct State<T> {
    queue: VecDeque<T>,
    senders: usize,
    receiver_open: bool,
}

pub(super) struct CommandSender<T>(Arc<Channel<T>>);

impl<T> Clone for CommandSender<T> {
    fn clone(&self) -> Self {
        self.0.state.lock().expect("command queue lock").senders += 1;
        Self(Arc::clone(&self.0))
    }
}

impl<T> Drop for CommandSender<T> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().expect("command queue lock");
        state.senders -= 1;
        let disconnected = state.senders == 0 && state.receiver_open;
        drop(state);
        if disconnected {
            self.0.wake.notify();
        }
    }
}

impl<T> CommandSender<T> {
    pub(super) fn send(&self, value: T) -> Result<(), SendError<T>> {
        let state = self.0.state.lock().expect("command queue lock");
        let state = self
            .0
            .available
            .wait_while(state, |state| {
                state.receiver_open && state.queue.len() == self.0.capacity
            })
            .expect("command queue lock");
        if !state.receiver_open {
            return Err(SendError(value));
        }
        self.push(state, value);
        Ok(())
    }

    pub(super) fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        let state = self.0.state.lock().expect("command queue lock");
        if !state.receiver_open {
            return Err(TrySendError::Disconnected(value));
        }
        if state.queue.len() == self.0.capacity {
            return Err(TrySendError::Full(value));
        }
        self.push(state, value);
        Ok(())
    }

    fn push(&self, mut state: std::sync::MutexGuard<'_, State<T>>, value: T) {
        let needs_wake = state.queue.is_empty();
        state.queue.push_back(value);
        // Signal before another producer can observe a nonempty queue.
        // Capacity waiters have their own signal.
        if needs_wake {
            self.0.wake.notify();
        }
    }

    pub(super) fn wake(&self) -> ProcessWake {
        self.0.wake.clone()
    }
}

pub(super) struct CommandReceiver<T>(Arc<Channel<T>>);

impl<T> CommandReceiver<T> {
    pub(super) fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut state = self.0.state.lock().expect("command queue lock");
        let value = match state.queue.pop_front() {
            Some(value) => value,
            None if state.senders == 0 => return Err(TryRecvError::Disconnected),
            None => return Err(TryRecvError::Empty),
        };
        let was_full = state.queue.len() + 1 == self.0.capacity;
        if state.queue.is_empty() && state.queue.capacity() > REUSABLE_SLOTS {
            state.queue = VecDeque::new();
        }
        drop(state);
        if was_full {
            // Several slots can be drained before any blocked sender wakes.
            self.0.available.notify_all();
        }
        Ok(value)
    }

    #[cfg(test)]
    pub(super) fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<T, std::sync::mpsc::RecvTimeoutError> {
        use std::{sync::mpsc::RecvTimeoutError, time::Instant};
        let deadline = Instant::now() + timeout;
        loop {
            let revision = self.0.wake.revision();
            match self.try_recv() {
                Ok(value) => return Ok(value),
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
                Err(TryRecvError::Empty) => {}
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(RecvTimeoutError::Timeout);
            };
            self.0.wake.wait(revision, Some(remaining));
        }
    }
}

impl<T> Drop for CommandReceiver<T> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().expect("command queue lock");
        state.receiver_open = false;
        let queue = std::mem::take(&mut state.queue);
        drop(state);
        self.0.available.notify_all();
        // Commands may own dispatchers that in turn own command senders.
        drop(queue);
    }
}

pub(super) fn channel<T>(
    capacity: usize,
    wake: &ProcessWake,
) -> (CommandSender<T>, CommandReceiver<T>) {
    assert!(capacity > 0, "command queue requires positive capacity");
    let shared = Arc::new(Channel {
        capacity,
        available: Condvar::new(),
        state: Mutex::new(State {
            queue: VecDeque::new(),
            senders: 1,
            receiver_open: true,
        }),
        wake: wake.clone(),
    });
    (CommandSender(Arc::clone(&shared)), CommandReceiver(shared))
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, thread, time::Duration};

    use super::*;

    #[test]
    fn queue_allocates_on_use_and_releases_drained_burst() {
        let capacity = 11;
        let (sender, receiver) = channel(capacity, &ProcessWake::default());
        assert_eq!(sender.0.state.lock().unwrap().queue.capacity(), 0);
        for value in 0..capacity {
            sender.try_send(value).unwrap();
        }
        assert!(
            matches!(sender.try_send(capacity), Err(TrySendError::Full(value)) if value == capacity)
        );
        drop(sender);
        for value in 0..capacity {
            assert_eq!(receiver.try_recv(), Ok(value));
        }
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Disconnected));
        assert_eq!(receiver.0.state.lock().unwrap().queue.capacity(), 0);
    }

    #[test]
    fn receiver_draining_or_closing_unblocks_a_full_sender() {
        for close in [false, true] {
            let (sender, receiver) = channel(1, &ProcessWake::default());
            sender.try_send(1).unwrap();
            let (result_tx, result_rx) = mpsc::channel();
            let writer = thread::spawn(move || result_tx.send(sender.send(2)).unwrap());
            assert_eq!(
                result_rx.recv_timeout(Duration::from_millis(20)),
                Err(mpsc::RecvTimeoutError::Timeout)
            );
            if close {
                drop(receiver);
                assert_eq!(
                    result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                    Err(SendError(2))
                );
            } else {
                assert_eq!(receiver.try_recv(), Ok(1));
                assert_eq!(
                    result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                    Ok(())
                );
                assert_eq!(receiver.try_recv(), Ok(2));
            }
            writer.join().unwrap();
        }
    }
}

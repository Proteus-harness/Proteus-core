use proteus_process_host::ProcessWake;
use std::sync::mpsc::{self, Receiver, SendError, SyncSender, TrySendError};

pub(super) struct CommandSender<T> {
    sender: SyncSender<T>,
    wake: ProcessWake,
}

impl<T> Clone for CommandSender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            wake: self.wake.clone(),
        }
    }
}

impl<T> CommandSender<T> {
    pub(super) fn send(&self, value: T) -> Result<(), SendError<T>> {
        self.sender.send(value)?;
        self.wake.notify();
        Ok(())
    }

    pub(super) fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        self.sender.try_send(value)?;
        self.wake.notify();
        Ok(())
    }

    pub(super) fn wake(&self) -> ProcessWake {
        self.wake.clone()
    }
}

pub(super) fn channel<T>(capacity: usize, wake: &ProcessWake) -> (CommandSender<T>, Receiver<T>) {
    let (sender, receiver) = mpsc::sync_channel(capacity);
    (
        CommandSender {
            sender,
            wake: wake.clone(),
        },
        receiver,
    )
}

use super::LifecycleCommand;
use std::{sync::mpsc, time::Duration};

#[cfg(target_os = "linux")]
mod linux;

#[derive(Debug)]
pub(super) struct MonitorWake {
    #[cfg(target_os = "linux")]
    poll: Option<linux::ChildPoll>,
}

impl MonitorWake {
    pub(super) fn new(_pid: u32) -> Self {
        Self {
            #[cfg(target_os = "linux")]
            poll: linux::ChildPoll::new(_pid).ok(),
        }
    }

    pub(super) fn notify(&self) {
        #[cfg(target_os = "linux")]
        if let Some(poll) = &self.poll {
            poll.notify();
        }
    }

    pub(super) fn receive(
        &self,
        commands: &mpsc::Receiver<LifecycleCommand>,
    ) -> Result<LifecycleCommand, mpsc::RecvTimeoutError> {
        #[cfg(target_os = "linux")]
        if let Some(poll) = &self.poll {
            match commands.try_recv() {
                Ok(command) => return Ok(command),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(mpsc::RecvTimeoutError::Disconnected);
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if poll.wait().is_ok() {
                return commands.try_recv().map_err(|error| match error {
                    mpsc::TryRecvError::Empty => mpsc::RecvTimeoutError::Timeout,
                    mpsc::TryRecvError::Disconnected => mpsc::RecvTimeoutError::Disconnected,
                });
            }
        }
        // Other platforms, older kernels and unavailable pidfds retain the
        // portable monitor; only a successful kernel subscription blocks forever.
        commands.recv_timeout(Duration::from_millis(10))
    }
}

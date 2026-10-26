use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

#[derive(Debug)]
pub(super) struct ChildPoll {
    child: OwnedFd,
    command: OwnedFd,
}

impl ChildPoll {
    pub(super) fn new(pid: u32) -> io::Result<Self> {
        // SAFETY: pid names the still-owned, unreaped child; flags are zero.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the syscall returned a new descriptor owned by this monitor.
        let child = unsafe { OwnedFd::from_raw_fd(fd as _) };
        // SAFETY: eventfd takes integer flags and returns a new descriptor.
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the descriptor is valid and has not been adopted elsewhere.
        let command = unsafe { OwnedFd::from_raw_fd(fd) };
        Ok(Self { child, command })
    }

    pub(super) fn notify(&self) {
        let value: u64 = 1;
        loop {
            // SAFETY: value is a live eight-byte eventfd payload; the fd is owned.
            let result =
                unsafe { libc::write(self.command.as_raw_fd(), (&value as *const u64).cast(), 8) };
            if result >= 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                break;
            }
        }
    }

    pub(super) fn wait(&self) -> io::Result<()> {
        let mut events = [
            libc::pollfd {
                fd: self.child.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.command.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        loop {
            // SAFETY: both pollfd entries and their owned descriptors remain live.
            let result = unsafe { libc::poll(events.as_mut_ptr(), events.len() as _, -1) };
            if result >= 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        if events
            .iter()
            .any(|event| event.revents & libc::POLLNVAL != 0)
        {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        if events[1].revents & libc::POLLIN != 0 {
            let mut value: u64 = 0;
            loop {
                // SAFETY: value is writable eight-byte storage for eventfd's counter.
                let result = unsafe {
                    libc::read(self.command.as_raw_fd(), (&mut value as *mut u64).cast(), 8)
                };
                if result >= 0 {
                    break;
                }
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock {
                    break;
                }
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

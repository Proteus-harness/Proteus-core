//! In-memory byte streams: exercise production framing without socket privileges.
use std::{
    io::{self, Cursor, Read, Write},
    sync::mpsc,
    time::Duration,
};

pub struct Reader {
    receiver: mpsc::Receiver<Vec<u8>>,
    buffered: Cursor<Vec<u8>>,
    timeout: bool,
}

pub struct Writer(mpsc::Sender<Vec<u8>>);

pub fn stream() -> (Reader, Writer) {
    let (sender, receiver) = mpsc::channel();
    (
        Reader {
            receiver,
            buffered: Cursor::new(Vec::new()),
            timeout: false,
        },
        Writer(sender),
    )
}

impl Reader {
    pub fn with_timeout(mut self) -> Self {
        self.timeout = true;
        self
    }
}

impl Read for Reader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            let size = self.buffered.read(bytes)?;
            if size != 0 {
                return Ok(size);
            }
            let message = if self.timeout {
                match self.receiver.recv_timeout(Duration::from_secs(5)) {
                    Ok(bytes) => bytes,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(0),
                    Err(error) => return Err(io::Error::new(io::ErrorKind::TimedOut, error)),
                }
            } else {
                match self.receiver.recv() {
                    Ok(bytes) => bytes,
                    Err(_) => return Ok(0),
                }
            };
            self.buffered = Cursor::new(message);
        }
    }
}

impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .send(bytes.to_vec())
            .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

use std::{
    collections::HashMap,
    io::{BufRead, BufWriter, Read, Write},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
};

use anyhow::{Context, Result, anyhow, bail};
use proteus_contracts::contracts::ProcessModuleCallbackParams;
use proteus_module_protocol::ProcessModuleRpcError;
use serde_json::{Value, json};

const MAX_PENDING_CALLBACKS: usize = 256;

pub struct FrameReader {
    reader: Box<dyn BufRead>,
    max_frame_bytes: usize,
}

impl FrameReader {
    pub fn new(reader: impl BufRead + 'static, max_frame_bytes: usize) -> Self {
        Self {
            reader: Box::new(reader),
            max_frame_bytes,
        }
    }

    pub fn read(&mut self) -> Result<Option<Value>> {
        let mut line = Vec::new();
        loop {
            line.clear();
            if (&mut self.reader)
                .take(self.max_frame_bytes as u64 + 1)
                .read_until(b'\n', &mut line)?
                == 0
            {
                return Ok(None);
            }
            if line.len() > self.max_frame_bytes {
                bail!("worker input frame exceeded {} bytes", self.max_frame_bytes);
            }
            let line = std::str::from_utf8(&line).context("worker received non-UTF8 JSON")?;
            if !line.trim().is_empty() {
                return serde_json::from_str(&line)
                    .context("worker received malformed newline JSON")
                    .map(Some);
            }
        }
    }
}

type CallbackResult = Result<Value, ProcessModuleRpcError>;

/// Shared output and callback routing. stdin is intentionally absent: only the
/// dispatch loop owns the reader, while invocation threads wait on their own
/// callback channels.
pub struct ModuleTransport {
    generation: u64,
    closed: AtomicBool,
    writer: Mutex<BufWriter<Box<dyn Write + Send>>>,
    callbacks: Mutex<HashMap<String, mpsc::SyncSender<CallbackResult>>>,
    next_callback_sequence: AtomicU64,
    max_frame_bytes: usize,
}

impl ModuleTransport {
    pub fn new(
        generation: u64,
        writer: impl Write + Send + 'static,
        max_frame_bytes: usize,
    ) -> Self {
        Self {
            generation,
            closed: AtomicBool::new(false),
            writer: Mutex::new(BufWriter::new(Box::new(writer))),
            callbacks: Mutex::new(HashMap::new()),
            next_callback_sequence: AtomicU64::new(1),
            max_frame_bytes,
        }
    }

    pub fn write(&self, value: &Value) -> Result<()> {
        // Reject an oversized result before writing any part of its frame.
        // Counting streams the canonical JSON without retaining another payload.
        let mut counter = FrameByteCounter {
            remaining: self.max_frame_bytes.saturating_sub(1),
        };
        serde_json::to_writer(&mut counter, value)?;
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("worker writer lock poisoned"))?;
        serde_json::to_writer(&mut *writer, value)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        Ok(())
    }

    pub fn call_host(&self, invocation_id: &str, method: &str, params: Value) -> Result<Value> {
        let sequence = self.next_callback_sequence.fetch_add(1, Ordering::Relaxed);
        if sequence == 0 {
            bail!("callback id sequence overflowed");
        }
        let id = format!("m:{}:{sequence}", self.generation);
        let (sender, receiver) = mpsc::sync_channel(1);
        {
            let mut callbacks = self
                .callbacks
                .lock()
                .map_err(|_| anyhow!("worker callback map poisoned"))?;
            if self.is_closed() {
                bail!("component transport is closed");
            }
            if callbacks.len() >= MAX_PENDING_CALLBACKS {
                bail!("worker pending callback capacity is exhausted");
            }
            if callbacks.insert(id.clone(), sender).is_some() {
                bail!("worker callback id was reused: {id}");
            }
        }
        let frame = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": ProcessModuleCallbackParams {
                invocation_id: invocation_id.to_owned(),
                params,
            }
        });
        if let Err(error) = self.write(&frame) {
            self.remove_callback(&id);
            return Err(error).context("failed to write host callback request");
        }
        receiver
            .recv()
            .map_err(|_| anyhow!("host callback {id} was abandoned"))?
            .map_err(anyhow::Error::from)
    }

    pub fn complete_callback(&self, id: &str, result: CallbackResult) -> Result<()> {
        let sender = self
            .callbacks
            .lock()
            .map_err(|_| anyhow!("worker callback map poisoned"))?
            .remove(id)
            .ok_or_else(|| anyhow!("callback response references unknown id {id:?}"))?;
        sender
            .send(result)
            .map_err(|_| anyhow!("callback waiter {id:?} disappeared"))
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Close under the callback-map lock, so a concurrent invocation cannot
    /// enqueue a waiter after EOF has drained all pending callback senders.
    pub fn close(&self) {
        if let Ok(mut callbacks) = self.callbacks.lock() {
            self.closed.store(true, Ordering::SeqCst);
            callbacks.clear();
        }
    }

    fn remove_callback(&self, id: &str) {
        if let Ok(mut callbacks) = self.callbacks.lock() {
            callbacks.remove(id);
        }
    }
}

struct FrameByteCounter {
    remaining: usize,
}

impl Write for FrameByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(std::io::Error::other(
                "worker output frame exceeded its configured byte budget",
            ));
        }
        self.remaining -= bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn rpc_success(id: &str, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

pub fn rpc_error(id: &str, error: ProcessModuleRpcError) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn oversized_output_is_rejected_before_any_frame_bytes_are_written() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let transport = ModuleTransport::new(1, Capture(bytes.clone()), 64);
        assert!(transport.write(&json!({"large":"x".repeat(64)})).is_err());
        assert!(bytes.lock().unwrap().is_empty());
        transport.write(&json!({"ok":true})).unwrap();
        assert_eq!(&*bytes.lock().unwrap(), b"{\"ok\":true}\n");
    }
}

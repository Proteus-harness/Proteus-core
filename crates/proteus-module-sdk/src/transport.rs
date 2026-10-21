use std::{
    collections::HashMap,
    io::{BufRead, BufWriter, Write},
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
}

impl FrameReader {
    pub fn new(reader: impl BufRead + 'static) -> Self {
        Self {
            reader: Box::new(reader),
        }
    }

    pub fn read(&mut self) -> Result<Option<Value>> {
        let mut line = String::new();
        loop {
            line.clear();
            if self.reader.read_line(&mut line)? == 0 {
                return Ok(None);
            }
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
}

impl ModuleTransport {
    pub fn new(generation: u64, writer: impl Write + Send + 'static) -> Self {
        Self {
            generation,
            closed: AtomicBool::new(false),
            writer: Mutex::new(BufWriter::new(Box::new(writer))),
            callbacks: Mutex::new(HashMap::new()),
            next_callback_sequence: AtomicU64::new(1),
        }
    }

    pub fn write(&self, value: &Value) -> Result<()> {
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

pub fn rpc_success(id: &str, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

pub fn rpc_error(id: &str, error: ProcessModuleRpcError) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}

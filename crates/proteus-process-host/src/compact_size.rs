use std::io::{self, Write};

use anyhow::Result;
use serde_json::Value;

/// Exact compact JSON body length for the transport queue budgets.
/// The raw wire length and frame terminators belong to the framing layer.
pub(crate) fn compact_json_len(value: &Value) -> Result<usize> {
    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.bytes)
}

#[derive(Default)]
struct ByteCounter {
    bytes: usize,
}

impl Write for ByteCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.checked_add(buffer.len()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::FileTooLarge, "JSON frame length overflow")
        })?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;

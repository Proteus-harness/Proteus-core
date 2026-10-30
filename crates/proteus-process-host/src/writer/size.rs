use std::io::{self, Write};

use anyhow::Result;
use serde_json::Value;

/// Exact compact JSON size for the already-owned transport Value.
///
/// This is not a generic Serialize size hint: it mirrors the six Value variants
/// and serde_json's default compact string escaping. Number formatting remains
/// delegated to serde_json, including feature-dependent number representations.
/// The entire Value is measured before the writer reserves any queue bytes.
pub(super) fn compact_json_len(value: &Value) -> Result<usize> {
    let mut size = JsonSize::default();
    size.value(value)?;
    Ok(size.bytes)
}

#[derive(Default)]
struct JsonSize {
    bytes: usize,
}

impl JsonSize {
    fn add(&mut self, bytes: usize) -> io::Result<()> {
        self.bytes = self.bytes.checked_add(bytes).ok_or_else(|| {
            io::Error::new(io::ErrorKind::FileTooLarge, "JSON frame length overflow")
        })?;
        Ok(())
    }

    fn string(&mut self, value: &str) -> io::Result<()> {
        self.add(2)?; // Quotes.
        self.add(value.len())?; // UTF-8, not Unicode scalar count.
        for byte in value.bytes() {
            match byte {
                b'"' | b'\\' | b'\x08' | b'\t' | b'\n' | b'\x0c' | b'\r' => self.add(1)?,
                0x00..=0x1f => self.add(5)?, // Six-byte \u00XX replaces one byte.
                _ => {}
            }
        }
        Ok(())
    }

    fn value(&mut self, value: &Value) -> Result<()> {
        match value {
            Value::Null => self.add(4)?,
            Value::Bool(value) => self.add(if *value { 4 } else { 5 })?,
            Value::Number(number) => serde_json::to_writer(&mut *self, number)?,
            Value::String(value) => self.string(value)?,
            Value::Array(values) => {
                self.add(2)?; // Brackets.
                self.add(values.len().saturating_sub(1))?; // Commas.
                for value in values {
                    self.value(value)?;
                }
            }
            Value::Object(values) => {
                self.add(2)?; // Braces.
                self.add(values.len().saturating_sub(1))?; // Commas.
                for (key, value) in values {
                    self.string(key)?;
                    self.add(1)?; // Colon.
                    self.value(value)?;
                }
            }
        }
        Ok(())
    }
}

impl Write for JsonSize {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.add(buffer.len())?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;

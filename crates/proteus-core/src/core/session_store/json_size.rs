//! Compact JSON byte length without retaining an encoded copy. A rejected value
//! never yields a partial count. Cache callers supply their existing hard limit.
use std::io::Write;

use serde::Serialize;

pub(super) fn encoded_len(value: &impl Serialize, limit: usize) -> Option<usize> {
    struct Counter {
        used: usize,
        limit: usize,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.used) {
                return Err(std::io::Error::other("encoded JSON exceeds byte budget"));
            }
            self.used += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { used: 0, limit };
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.used)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cache_limit_is_inclusive_and_counts_escaped_utf8_bytes() {
        for value in [json!(null), json!({"ключ": ["\n\t\0\\\"", "🦀", -1.5]})] {
            let bytes = serde_json::to_vec(&value).unwrap().len();
            assert_eq!(encoded_len(&value, bytes), Some(bytes));
            assert_eq!(encoded_len(&value, bytes - 1), None);
        }
    }
}

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    time::Instant,
};

pub fn memory() -> Result<Value> {
    // Self-only proc reads: no privilege or access to another user's processes.
    let rollup = fs::read_to_string("/proc/self/smaps_rollup")?;
    let status = fs::read_to_string("/proc/self/status")?;
    let field = |source: &str, key: &str| -> Result<u64> {
        source
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .context(format!("missing proc field {key}"))?
            .split_whitespace()
            .next()
            .context("missing value")?
            .parse()
            .context("invalid proc value")
    };
    let pss = field(&rollup, "Pss:")?;
    let swap = field(&rollup, "SwapPss:")?;
    Ok(json!({
        "pss_kib": pss, "swap_pss_kib": swap, "footprint_kib": pss + swap,
        "rss_kib": field(&rollup, "Rss:")?,
        "private_kib": field(&rollup, "Private_Clean:")? + field(&rollup, "Private_Dirty:")?,
        "peak_rss_kib": field(&status, "VmHWM:")?,
    }))
}

pub struct Read {
    started: Instant,
    pub report: Value,
    count: i64,
    digest: u64,
}

impl Read {
    pub fn new() -> Result<Self> {
        let baseline = memory()?;
        Ok(Self {
            started: Instant::now(),
            report: json!({"memory": {"baseline": baseline}}),
            count: 0,
            digest: 0,
        })
    }

    pub fn opened(&mut self) -> Result<()> {
        self.report["open_ms"] = (self.started.elapsed().as_secs_f64() * 1000.).into();
        self.report["memory"]["open"] = memory()?;
        Ok(())
    }

    pub fn row(&mut self, seq: i64, payload: &str) -> Result<Value> {
        let value: Value = serde_json::from_str(payload)?;
        ensure!(
            value["seq"].as_i64() == Some(seq),
            "message identity mismatch"
        );
        let mut hash = DefaultHasher::new();
        seq.hash(&mut hash);
        payload.hash(&mut hash);
        self.digest = self.digest.wrapping_add(hash.finish());
        self.count += 1;
        Ok(value)
    }

    pub fn ready(&mut self, retained: &[Value], total: i64) -> Result<()> {
        ensure!(!retained.is_empty(), "empty history");
        let from = total - crate::fixture::PAGE;
        let mut last: Vec<_> = retained
            .iter()
            .filter(|v| v["seq"].as_i64().unwrap() >= from)
            .collect();
        last.sort_by_key(|v| v["seq"].as_i64().unwrap());
        ensure!(
            last.len() == crate::fixture::PAGE as usize,
            "incomplete tail page"
        );
        let mut hash = DefaultHasher::new();
        for (offset, row) in last.into_iter().enumerate() {
            ensure!(
                row["seq"].as_i64() == Some(from + offset as i64),
                "tail order mismatch"
            );
            row.to_string().hash(&mut hash);
        }
        self.report["tail_digest"] = hash.finish().into();
        self.report["ready_ms"] = (self.started.elapsed().as_secs_f64() * 1000.).into();
        self.report["retained_rows"] = retained.len().into();
        self.report["memory"]["ready"] = memory()?;
        std::hint::black_box(retained);
        Ok(())
    }

    pub fn finish(mut self, expected: i64) -> Result<Value> {
        ensure!(self.count == expected, "history length mismatch");
        self.report["visited_rows"] = self.count.into();
        self.report["history_digest"] = self.digest.into();
        self.report["scan_ms"] = (self.started.elapsed().as_secs_f64() * 1000.).into();
        self.report["memory"]["scanned"] = memory()?;
        Ok(self.report)
    }
}

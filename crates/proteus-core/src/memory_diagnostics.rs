//! Opt-in, content-free phase measurements for synthetic memory investigations.
//! mallinfo2 reports process-wide glibc totals, not Rust object ownership.
use std::{
    cell::Cell,
    fs::{File, OpenOptions},
    io::{Read, Write},
    marker::PhantomData,
    path::Path,
    rc::Rc,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Serialize, Serializer, ser::SerializeMap};

static TRACE: OnceLock<Option<Trace>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
thread_local! { static CURRENT: Cell<Option<u64>> = const { Cell::new(None) }; }

struct Trace {
    file: Mutex<File>,
    started: Instant,
}

fn trace() -> Option<&'static Trace> {
    TRACE
        .get_or_init(|| {
            let path = std::env::var_os("PROTEUS_MEMORY_TRACE")?;
            if !Path::new(&path).is_absolute() {
                return None;
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()?;
            Some(Trace {
                file: Mutex::new(file),
                started: Instant::now(),
            })
        })
        .as_ref()
}

/// Must stay within one synchronous scope. The !Send marker prevents moving a
/// thread-local parent across a Tokio migration; never hold this over an await.
pub(crate) struct Scope {
    trace: Option<&'static Trace>,
    id: u64,
    parent: Option<u64>,
    phase: &'static str,
    _same_thread: PhantomData<Rc<()>>,
}

impl Scope {
    pub(crate) fn new(phase: &'static str) -> Self {
        let trace = trace();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let parent = trace.and_then(|_| CURRENT.with(|current| current.replace(Some(id))));
        let scope = Self {
            trace,
            id,
            parent,
            phase,
            _same_thread: PhantomData,
        };
        scope.sample_event("begin", phase, &[]);
        scope
    }

    pub(crate) fn sample(&self, phase: &'static str, counts: &[(&'static str, usize)]) {
        self.sample_event("sample", phase, counts);
    }

    fn sample_event(
        &self,
        event: &'static str,
        phase: &'static str,
        counts: &[(&'static str, usize)],
    ) {
        if let Some(trace) = self.trace {
            emit(trace, Some(self.id), self.parent, event, phase, counts);
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        self.sample_event("end", self.phase, &[]);
        if self.trace.is_some() {
            CURRENT.with(|current| current.set(self.parent));
        }
    }
}

pub(crate) fn point(phase: &'static str, counts: &[(&'static str, usize)]) {
    if let Some(trace) = trace() {
        emit(trace, None, CURRENT.with(Cell::get), "point", phase, counts);
    }
}

#[derive(Default, Serialize)]
struct Resident {
    rss_bytes: Option<u64>,
    vm_hwm_bytes: Option<u64>,
}

fn resident() -> Resident {
    let mut snapshot = Resident::default();
    #[cfg(target_os = "linux")]
    {
        // No heap buffer: sample the target before formatting the trace record.
        let mut buffer = [0_u8; 8192];
        if let Ok(mut file) = File::open("/proc/self/status") {
            let mut used = 0;
            while used < buffer.len() {
                match file.read(&mut buffer[used..]) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => used += count,
                }
            }
            if let Ok(text) = std::str::from_utf8(&buffer[..used]) {
                for line in text.lines() {
                    let (destination, value) = if let Some(value) = line.strip_prefix("VmRSS:") {
                        (&mut snapshot.rss_bytes, value)
                    } else if let Some(value) = line.strip_prefix("VmHWM:") {
                        (&mut snapshot.vm_hwm_bytes, value)
                    } else {
                        continue;
                    };
                    *destination = value
                        .split_whitespace()
                        .next()
                        .and_then(|value| value.parse::<u64>().ok())
                        .and_then(|value| value.checked_mul(1024));
                }
            }
        }
    }
    snapshot
}

#[derive(Serialize)]
struct GlibcTotals {
    arena_bytes: usize,
    uordblks_bytes: usize,
    fordblks_bytes: usize,
    hblkhd_bytes: usize,
}

fn glibc_totals() -> Option<GlibcTotals> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        #[repr(C)]
        struct Mallinfo2 {
            arena: usize,
            ordblks: usize,
            smblks: usize,
            hblks: usize,
            hblkhd: usize,
            usmblks: usize,
            fsmblks: usize,
            uordblks: usize,
            fordblks: usize,
            keepcost: usize,
        }
        unsafe extern "C" {
            fn mallinfo2() -> Mallinfo2;
        }
        // glibc returns an allocator-wide snapshot and does not mutate objects.
        let info = unsafe { mallinfo2() };
        Some(GlibcTotals {
            arena_bytes: info.arena,
            uordblks_bytes: info.uordblks,
            fordblks_bytes: info.fordblks,
            hblkhd_bytes: info.hblkhd,
        })
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        None
    }
}

struct Counts<'a>(&'a [(&'static str, usize)]);
impl Serialize for Counts<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

#[derive(Serialize)]
struct Record<'a> {
    id: u64,
    scope_id: Option<u64>,
    parent_scope_id: Option<u64>,
    pid: u32,
    elapsed_us: u128,
    wall_time_ns: Option<u128>,
    event: &'static str,
    phase: &'static str,
    #[serde(flatten)]
    resident: Resident,
    glibc_allocator_totals_proxy: Option<GlibcTotals>,
    counts: Counts<'a>,
}

fn emit(
    trace: &Trace,
    scope_id: Option<u64>,
    parent_scope_id: Option<u64>,
    event: &'static str,
    phase: &'static str,
    counts: &[(&'static str, usize)],
) {
    let resident = resident();
    let glibc_allocator_totals_proxy = glibc_totals();
    let record = Record {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        scope_id,
        parent_scope_id,
        pid: std::process::id(),
        elapsed_us: trace.started.elapsed().as_micros(),
        wall_time_ns: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|elapsed| elapsed.as_nanos()),
        event,
        phase,
        resident,
        glibc_allocator_totals_proxy,
        counts: Counts(counts),
    };
    // Format only after sampling, then append one small line. Child backends may
    // inherit the trace path, so do not interleave individual serializer writes.
    if let Ok(mut line) = serde_json::to_vec(&record) {
        line.push(b'\n');
        if let Ok(mut file) = trace.file.lock() {
            // Diagnostic IO failures must never change the operation result.
            let _ = file.write_all(&line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_free_record_is_jsonl_with_explicit_allocator_proxy() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("trace.jsonl");
        let trace = Trace {
            file: Mutex::new(File::create(&path).unwrap()),
            started: Instant::now(),
        };
        emit(
            &trace,
            Some(7),
            Some(3),
            "sample",
            "test.phase",
            &[("records", 11)],
        );
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 1);
        let record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(record["scope_id"], 7);
        assert_eq!(record["parent_scope_id"], 3);
        assert_eq!(record["pid"], std::process::id());
        assert_eq!(record["phase"], "test.phase");
        assert!(
            record["wall_time_ns"]
                .as_u64()
                .is_some_and(|value| value > 0)
        );
        assert_eq!(record["counts"]["records"], 11);
        assert!(record.get("glibc_allocator_totals_proxy").is_some());
        #[cfg(target_os = "linux")]
        assert!(record["rss_bytes"].as_u64().is_some_and(|rss| rss > 0));
    }

    #[test]
    fn unwritable_trace_does_not_fail_the_runtime_operation() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let trace = Trace {
            file: Mutex::new(File::open(file.path()).unwrap()),
            started: Instant::now(),
        };
        emit(&trace, None, None, "point", "test.read_only_file", &[]);
        assert_eq!(file.as_file().metadata().unwrap().len(), 0);
    }
}

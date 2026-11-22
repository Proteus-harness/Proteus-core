//! Opt-in numerical spans for the process-backed startup dependency chain.
use std::{
    fs::File,
    io::Write,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const LIMIT: usize = 12;
static TRACE: OnceLock<Option<Trace>> = OnceLock::new();

struct Sink {
    file: File,
    remaining: usize,
}

struct Trace {
    origin: Instant,
    sink: Mutex<Sink>,
}

#[derive(Clone, Copy)]
pub(super) enum Phase {
    ModelInitialize,
    ModelDescribe,
    ContextInitialize,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::ModelInitialize => "model_initialize",
            Self::ModelDescribe => "model_describe",
            Self::ContextInitialize => "context_initialize",
        }
    }
}

#[cfg(unix)]
struct PrivateTraceDir(File);

#[cfg(unix)]
fn private_dir(meta: &std::fs::Metadata, euid: libc::uid_t) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.is_dir() && meta.uid() == euid && meta.mode() & 0o7777 == 0o700
}

#[cfg(unix)]
impl PrivateTraceDir {
    fn open(path: &std::path::Path) -> Option<Self> {
        use std::{
            fs::OpenOptions,
            os::unix::fs::OpenOptionsExt,
            path::{Component, PathBuf},
        };

        if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
            return None;
        }
        // Drop trailing slash/dot aliases so O_NOFOLLOW applies to the final
        // directory component. Authorization still comes from the opened fd.
        let path: PathBuf = path.components().collect();
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .ok()?;
        // SAFETY: geteuid has no arguments or memory preconditions.
        let euid = unsafe { libc::geteuid() };
        private_dir(&dir.metadata().ok()?, euid).then_some(Self(dir))
    }

    fn create(&self) -> Option<File> {
        use std::os::fd::{AsRawFd, FromRawFd};

        // SAFETY: the checked directory fd is kept alive, the basename is a
        // static C string, and O_CREAT receives a promoted integer mode.
        // Lookup and creation use that very fd, not the original directory path.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                c"core-startup.jsonl".as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600 as libc::c_int,
            )
        };
        if fd < 0 {
            return None;
        }
        // SAFETY: successful openat transfers ownership of this fresh fd.
        Some(unsafe { File::from_raw_fd(fd) })
    }
}

#[cfg(unix)]
fn new_trace() -> Option<Trace> {
    let path = std::env::var_os("PROTEUS_CORE_STARTUP_TRACE_DIR")?;
    let file = PrivateTraceDir::open(std::path::Path::new(&path))?.create()?;
    Some(Trace {
        origin: Instant::now(),
        sink: Mutex::new(Sink {
            file,
            remaining: LIMIT,
        }),
    })
}

#[cfg(not(unix))]
fn new_trace() -> Option<Trace> {
    None
}

fn micros(value: Duration) -> u64 {
    u64::try_from(value.as_micros()).unwrap_or(u64::MAX)
}

fn line(phase: Phase, start_us: u64, duration_us: u64, ok: bool) -> String {
    format!(
        "{{\"phase\":\"{}\",\"start_us\":{start_us},\"duration_us\":{duration_us},\"ok\":{ok}}}\n",
        phase.label(),
    )
}

pub(super) fn measure<T, E>(phase: Phase, call: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
    let Some(trace) = TRACE.get_or_init(new_trace).as_ref() else {
        return call();
    };
    let start = Instant::now();
    let result = call();
    // Sample before any formatting or logging. Trace IO never changes the RPC result.
    let duration = micros(start.elapsed());
    if let Ok(mut sink) = trace.sink.lock()
        && sink.remaining > 0
    {
        sink.remaining -= 1;
        let record = line(phase, micros(start.duration_since(trace.origin)), duration, result.is_ok());
        if sink.file.write_all(record.as_bytes()).is_err() {
            sink.remaining = 0;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn trace_output_uses_the_checked_directory_fd_after_rename() {
        use std::{fs, os::unix::fs::{DirBuilderExt, PermissionsExt}};

        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let held = root.path().join("held");
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700).create(&source).unwrap();
        let dir = PrivateTraceDir::open(&source).unwrap();
        fs::rename(&source, &held).unwrap();
        builder.create(&source).unwrap();
        let mut file = dir.create().unwrap();
        file.write_all(b"fixed fixture\n").unwrap();
        let result = held.join("core-startup.jsonl");
        assert_eq!(fs::read(&result).unwrap(), b"fixed fixture\n");
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o077, 0);
        assert!(!source.join("core-startup.jsonl").exists());
        assert!(dir.create().is_none());
        assert_eq!(fs::read(&result).unwrap(), b"fixed fixture\n");
    }

    #[cfg(unix)]
    #[test]
    fn trace_sink_declines_nonprivate_wrong_owner_and_symlink_targets() {
        use std::{fs, os::unix::fs::{symlink, PermissionsExt}};

        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        fs::create_dir(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        let meta = fs::metadata(&private).unwrap();
        // SAFETY: geteuid has no arguments or memory preconditions.
        let euid = unsafe { libc::geteuid() };
        assert!(private_dir(&meta, euid));
        assert!(!private_dir(&meta, euid.wrapping_add(1)));

        let alias = root.path().join("alias");
        symlink(&private, &alias).unwrap();
        assert!(PrivateTraceDir::open(&alias).is_none());
        assert!(PrivateTraceDir::open(&alias.join(".")).is_none());
        let sentinel = root.path().join("sentinel");
        fs::write(&sentinel, b"keep").unwrap();
        symlink(&sentinel, private.join("core-startup.jsonl")).unwrap();
        let dir = PrivateTraceDir::open(&private).unwrap();
        assert!(dir.create().is_none());
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");

        fs::set_permissions(&private, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(PrivateTraceDir::open(&private).is_none());
    }

    #[test]
    fn trace_has_only_fixed_numeric_fields_and_bounded_framing() {
        for phase in [Phase::ModelInitialize, Phase::ModelDescribe, Phase::ContextInitialize] {
            for ok in [true, false] {
                let row = line(phase, u64::MAX, u64::MAX, ok);
                assert!(row.is_ascii() && row.len() <= 128);
                assert_eq!(row.bytes().filter(|b| *b == b'\n').count(), 1);
                assert!(row.ends_with('\n'));
                let json: serde_json::Value = serde_json::from_str(&row).unwrap();
                assert_eq!(
                    json,
                    serde_json::json!({
                        "phase": phase.label(),
                        "start_us": u64::MAX,
                        "duration_us": u64::MAX,
                        "ok": ok,
                    }),
                );
            }
        }
        assert_eq!(micros(Duration::MAX), u64::MAX);
    }
}

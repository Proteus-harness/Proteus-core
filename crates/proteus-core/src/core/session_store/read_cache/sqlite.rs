//! Coherent disposable SQLite indexes; never written by a journal reader.
use super::*;
use rusqlite::{Connection, OpenFlags, params};

mod update;
pub(in crate::core::session_store) use update::Updater;

const FORMAT: i64 = 1;
const SCHEMA: &str = "CREATE TABLE records(section TEXT NOT NULL,position INTEGER NOT NULL,
    payload BLOB NOT NULL,checksum BLOB NOT NULL,PRIMARY KEY(section,position)) WITHOUT ROWID";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    format: i64,
    journal_schema: u32,
    fingerprint: SourceFingerprint,
}

pub(in crate::core::session_store) struct Reader {
    conn: Connection,
    dir: PathBuf,
    before: SourceFingerprint,
    source: SourceFingerprint,
}

impl Reader {
    pub(in crate::core::session_store) fn open(dir: &Path, name: &str) -> Option<Self> {
        // Pin the SQLite snapshot before a replacement may remove a hot
        // rollback journal. Existing readers keep their inode after rename.
        let _opening = read_open_lock(dir, name)?;
        Self::open_inner(dir, name)
    }

    fn open_inner(dir: &Path, name: &str) -> Option<Self> {
        let before = SourceFingerprint::read(dir, MAX_BLOBS)?;
        let path = cache_path(dir, name)?;
        if !fs::symlink_metadata(&path).ok()?.is_file() {
            return None;
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;
        conn.busy_timeout(std::time::Duration::from_millis(25))
            .ok()?;
        conn.execute_batch("PRAGMA cache_size=-2048; PRAGMA mmap_size=0; BEGIN")
            .ok()?;
        let mut reader = Self {
            conn,
            dir: dir.into(),
            source: before.clone(),
            before,
        };
        let source: Source = reader.one("source")?;
        if source.format != FORMAT || source.journal_schema != JOURNAL_SCHEMA_VERSION {
            return None;
        }
        reader.source = source.fingerprint;
        Some(reader)
    }

    pub(in crate::core::session_store) fn one<T: DeserializeOwned>(
        &self,
        section: &str,
    ) -> Option<T> {
        self.page(section, 0, 1)?.pop()
    }

    pub(in crate::core::session_store) fn page<T: DeserializeOwned>(
        &self,
        section: &str,
        start: usize,
        end: usize,
    ) -> Option<Vec<T>> {
        read_page(&self.conn, section, start, end)
    }

    pub(in crate::core::session_store) fn revision(&self) -> Option<String> {
        self.source.revision()
    }

    pub(in crate::core::session_store) fn row_matches(
        &self,
        section: &str,
        position: usize,
        hash: &[u8],
    ) -> Option<bool> {
        row_matches(&self.conn, section, position, hash)
    }

    pub(in crate::core::session_store) fn finish(self) -> Option<(SourceFingerprint, bool)> {
        let after = SourceFingerprint::read(&self.dir, MAX_BLOBS)?;
        let fresh = self.source == self.before && after == self.before;
        Some((self.source, fresh))
    }
}

pub(in crate::core::session_store) struct Writer {
    conn: Option<Connection>,
    dir: PathBuf,
    path: PathBuf,
    temp: PathBuf,
    source: SourceFingerprint,
    _lock: fs::File,
}

impl Writer {
    pub(in crate::core::session_store) fn previous(&self, name: &str) -> Option<Reader> {
        if cache_path(&self.dir, name)? != self.path {
            return None;
        }
        // This writer already owns the exclusive replacement gate.
        Reader::open_inner(&self.dir, name)
    }

    pub(in crate::core::session_store) fn new(
        dir: &Path,
        name: &str,
        source: &SourceFingerprint,
    ) -> Option<Self> {
        let path = cache_path(dir, name)?;
        let lock = write_lock(dir, name)?;
        if SourceFingerprint::read(dir, MAX_BLOBS).as_ref() != Some(source) {
            return None;
        }
        let temp = dir.join(format!(".read-{name}.tmp.{}", new_record_id()));
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)
            .ok()?;
        // Create the guard before SQLite initialization so every failure removes the candidate.
        let mut writer = Self {
            conn: None,
            dir: dir.into(),
            path,
            temp,
            source: source.clone(),
            _lock: lock,
        };
        writer.conn = Some(Connection::open(&writer.temp).ok()?);
        let conn = writer.conn.as_ref()?;
        // This is a replacement snapshot, not an append writer: no persistent WAL/SHM files.
        conn.execute_batch(
            "PRAGMA cache_size=-2048; PRAGMA mmap_size=0;
            PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; BEGIN IMMEDIATE",
        )
        .ok()?;
        conn.execute_batch(SCHEMA).ok()?;
        writer.one(
            "source",
            &Source {
                format: FORMAT,
                journal_schema: JOURNAL_SCHEMA_VERSION,
                fingerprint: source.clone(),
            },
        )?;
        Some(writer)
    }

    pub(in crate::core::session_store) fn one<T: Serialize>(
        &self,
        section: &str,
        data: &T,
    ) -> Option<()> {
        self.row(section, 0, data).map(|_| ())
    }

    pub(in crate::core::session_store) fn row<T: Serialize>(
        &self,
        section: &str,
        position: usize,
        data: &T,
    ) -> Option<Vec<u8>> {
        let bytes = serialize_bounded(data)?;
        let hash = digest(&SHA256, &bytes).as_ref().to_vec();
        self.conn
            .as_ref()?
            .execute(
                "INSERT INTO records VALUES(?1,?2,?3,?4)",
                params![section, i64::try_from(position).ok()?, &bytes, &hash],
            )
            .ok()?;
        Some(hash)
    }

    pub(in crate::core::session_store) fn finish(mut self) -> Option<()> {
        self.conn.as_ref()?.execute_batch("COMMIT").ok()?;
        self.conn.take()?.close().ok()?;
        fs::File::open(&self.temp).ok()?.sync_all().ok()?;
        if SourceFingerprint::read(&self.dir, MAX_BLOBS).as_ref() != Some(&self.source) {
            return None;
        }
        // A failed in-place transaction normally rolls back on close. If a
        // process died with a damaged database, its disposable rollback journal
        // must not be applied to the replacement generation. The cache lock
        // excludes another live writer while the generation is replaced.
        let mut journal = self.path.as_os_str().to_owned();
        journal.push("-journal");
        match fs::remove_file(PathBuf::from(journal)) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => return None,
        }
        fs::rename(&self.temp, &self.path).ok()?;
        Some(())
    }
}

fn lock_file(dir: &Path, name: &str) -> Option<fs::File> {
    cache_path(dir, name)?;
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(dir.join(format!(".read-{name}.lock")))
        .ok()?;
    Some(lock)
}

fn write_lock(dir: &Path, name: &str) -> Option<fs::File> {
    let lock = lock_file(dir, name)?;
    lock.try_lock().ok()?;
    Some(lock)
}

fn update_lock(dir: &Path, name: &str) -> Option<fs::File> {
    let lock = lock_file(dir, name)?;
    // SQLite's BEGIN IMMEDIATE serializes in-place writers. A shared gate
    // excludes replacement while allowing readers to open the old snapshot.
    lock.try_lock_shared().ok()?;
    Some(lock)
}

fn read_open_lock(dir: &Path, name: &str) -> Option<fs::File> {
    cache_path(dir, name)?;
    // Reading never creates coordination metadata. An older disposable cache
    // without a gate is rebuilt through the normal cache-miss path.
    let lock = fs::File::open(dir.join(format!(".read-{name}.lock"))).ok()?;
    lock.try_lock_shared().ok()?;
    Some(lock)
}

fn read_page<T: DeserializeOwned>(
    conn: &Connection,
    section: &str,
    start: usize,
    end: usize,
) -> Option<Vec<T>> {
    let mut statement = conn
        .prepare_cached(
            "SELECT position,payload,checksum FROM records
        WHERE section=?1 AND position>=?2 AND position<?3 ORDER BY position",
        )
        .ok()?;
    let mut rows = statement
        .query(params![
            section,
            i64::try_from(start).ok()?,
            i64::try_from(end).ok()?
        ])
        .ok()?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().ok()? {
        if row.get::<_, i64>(0).ok()? != i64::try_from(start + out.len()).ok()? {
            return None;
        }
        let bytes = row.get_ref(1).ok()?.as_blob().ok()?;
        let checksum = row.get_ref(2).ok()?.as_blob().ok()?;
        if bytes.len() > MAX_BYTES || digest(&SHA256, bytes).as_ref() != checksum {
            return None;
        }
        out.push(serde_json::from_slice(bytes).ok()?);
    }
    (out.len() == end.checked_sub(start)?).then_some(out)
}

fn row_matches(conn: &Connection, section: &str, position: usize, expected: &[u8]) -> Option<bool> {
    let mut statement = conn
        .prepare_cached("SELECT payload,checksum FROM records WHERE section=?1 AND position=?2")
        .ok()?;
    match statement.query_row(params![section, i64::try_from(position).ok()?], |row| {
        let bytes = row.get_ref(0)?;
        let checksum = row.get_ref(1)?;
        Ok(match (bytes.as_blob(), checksum.as_blob()) {
            (Ok(bytes), Ok(checksum)) => {
                bytes.len() <= MAX_BYTES
                    && checksum == expected
                    && digest(&SHA256, bytes).as_ref() == checksum
            }
            _ => false,
        })
    }) {
        Ok(matches) => Some(matches),
        Err(rusqlite::Error::QueryReturnedNoRows) => Some(false),
        Err(_) => None,
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.conn.take();
        let _ = fs::remove_file(&self.temp);
    }
}

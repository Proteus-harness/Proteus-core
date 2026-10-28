//! Transactional row updates preserve stable snapshots without WAL sidecars.
use super::*;
use std::time::Duration;

pub(in crate::core::session_store) struct Updater {
    conn: Option<Connection>,
    dir: PathBuf,
    source: SourceFingerprint,
    previous: SourceFingerprint,
    _lock: CacheGate,
}

impl Updater {
    pub(in crate::core::session_store) fn new(
        dir: &Path,
        name: &str,
        source: &SourceFingerprint,
    ) -> Option<Self> {
        let path = cache_path(dir, name)?;
        let lock = update_lock(dir, name)?;
        if !fs::symlink_metadata(&path).ok()?.is_file()
            || SourceFingerprint::read(dir, MAX_BLOBS).as_ref() != Some(source)
        {
            return None;
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;
        conn.busy_timeout(Duration::from_millis(25)).ok()?;
        // Readers keep the previous committed snapshot. If one outlives this
        // short commit window, the caller can atomically replace the cache.
        conn.execute_batch(
            "PRAGMA cache_size=-2048; PRAGMA mmap_size=0;
            PRAGMA synchronous=FULL; BEGIN IMMEDIATE",
        )
        .ok()?;
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .ok()?;
        if mode != "delete" {
            return None;
        }
        let previous: Source = read_page(&conn, "source", 0, 1)?.pop()?;
        if previous.format != FORMAT || previous.journal_schema != JOURNAL_SCHEMA_VERSION {
            return None;
        }
        Some(Self {
            conn: Some(conn),
            dir: dir.into(),
            source: source.clone(),
            previous: previous.fingerprint,
            _lock: lock,
        })
    }

    pub(in crate::core::session_store) fn previous<T: DeserializeOwned>(
        &self,
        section: &str,
    ) -> Option<T> {
        read_page(self.conn.as_ref()?, section, 0, 1)?.pop()
    }

    pub(in crate::core::session_store) fn revision(&self) -> Option<String> {
        self.previous.revision()
    }

    /// Returns true only when the existing payload and checksum both match.
    /// Unchanged rows avoid a full serialized payload allocation and SQL write.
    pub(in crate::core::session_store) fn row<T: Serialize>(
        &self,
        section: &str,
        position: usize,
        data: &T,
    ) -> Option<bool> {
        let expected = hash_serialized(data)?;
        let conn = self.conn.as_ref()?;
        if row_matches(conn, section, position, &expected)? {
            return Some(true);
        }
        let bytes = serialize_bounded(data)?;
        let hash = digest(&SHA256, &bytes);
        conn.prepare_cached(
            "INSERT INTO records VALUES(?1,?2,?3,?4)
            ON CONFLICT(section,position) DO UPDATE SET
                payload=excluded.payload, checksum=excluded.checksum",
        )
        .ok()?
        .execute(params![
            section,
            i64::try_from(position).ok()?,
            &bytes,
            hash.as_ref()
        ])
        .ok()?;
        Some(false)
    }

    pub(in crate::core::session_store) fn one<T: Serialize>(
        &self,
        section: &str,
        data: &T,
    ) -> Option<()> {
        self.row(section, 0, data).map(|_| ())
    }

    pub(in crate::core::session_store) fn truncate(
        &self,
        section: &str,
        length: usize,
    ) -> Option<()> {
        self.conn
            .as_ref()?
            .prepare_cached("DELETE FROM records WHERE section=?1 AND position>=?2")
            .ok()?
            .execute(params![section, i64::try_from(length).ok()?])
            .ok()?;
        Some(())
    }

    pub(in crate::core::session_store) fn finish(mut self) -> Option<()> {
        self.one(
            "source",
            &Source {
                format: FORMAT,
                journal_schema: JOURNAL_SCHEMA_VERSION,
                fingerprint: self.source.clone(),
            },
        )?;
        if SourceFingerprint::read(&self.dir, MAX_BLOBS).as_ref() != Some(&self.source) {
            return None;
        }
        self.conn.as_ref()?.execute_batch("COMMIT").ok()?;
        self.conn.take()?.close().ok()?;
        Some(())
    }
}

impl Drop for Updater {
    fn drop(&mut self) {
        // Closing a failed transaction rolls back before the write lock drops.
        self.conn.take();
    }
}

fn hash_serialized<T: Serialize>(data: &T) -> Option<[u8; 32]> {
    struct Hasher {
        digest: ring::digest::Context,
        bytes: usize,
    }
    impl Write for Hasher {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > MAX_BYTES.saturating_sub(self.bytes) {
                return Err(io::Error::other("read cache exceeds size limit"));
            }
            self.digest.update(bytes);
            self.bytes += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut hasher = Hasher {
        digest: ring::digest::Context::new(&SHA256),
        bytes: 0,
    };
    serde_json::to_writer(&mut hasher, data).ok()?;
    hasher.digest.finish().as_ref().try_into().ok()
}

#[cfg(test)]
mod tests;

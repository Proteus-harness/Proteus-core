use super::*;
use serde::ser::SerializeStruct;

fn source() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("session.json"), b"metadata").unwrap();
    fs::write(dir.path().join("journal.jsonl"), b"journal\n").unwrap();
    fs::create_dir(dir.path().join("blobs")).unwrap();
    fs::write(dir.path().join("blobs/payload.json"), b"payload").unwrap();
    dir
}
fn fingerprint(dir: &Path) -> SourceFingerprint {
    SourceFingerprint::read(dir, MAX_BLOBS).unwrap()
}

#[test]
fn serialized_reload_and_cache_writes_preserve_source_identity() {
    let dir = source();
    let before = fingerprint(dir.path());
    let data = vec!["one".to_owned(), "two".to_owned()];
    assert!(save(dir.path(), "test", &before, &data));
    assert_eq!(fingerprint(dir.path()), before);
    // A fresh reader needs only persisted bytes, not a process-local cache.
    let (loaded_source, loaded): (_, Vec<String>) = load(dir.path(), "test").unwrap();
    assert_eq!(loaded_source, before);
    assert_eq!(loaded, data);
    assert!(save(dir.path(), "other", &before, &data));
    assert_eq!(fingerprint(dir.path()), before);
    assert_eq!(load::<Vec<String>>(dir.path(), "test").unwrap().1, data);
}

#[test]
fn every_source_file_invalidates_even_with_same_size_and_preserved_mtime() {
    for name in ["session.json", "journal.jsonl", "blobs/payload.json"] {
        let dir = source();
        let before = fingerprint(dir.path());
        assert!(save(dir.path(), "test", &before, &123));
        let path = dir.path().join(name);
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 1;
        fs::write(&path, bytes).unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_ne!(fingerprint(dir.path()), before, "{name}");
        assert!(load::<i32>(dir.path(), "test").is_none(), "{name}");
        let (cached_source, data, fresh) = load_any::<i32>(dir.path(), "test").unwrap();
        assert_eq!(cached_source, before);
        assert_eq!(data, 123);
        assert!(!fresh, "{name}");
    }
}

#[test]
fn malformed_versions_and_checksum_are_disposable_misses() {
    let dir = source();
    let before = fingerprint(dir.path());
    let path = cache_path(dir.path(), "test").unwrap();
    for change in [
        "malformed",
        "schema_version",
        "journal_schema_version",
        "checksum",
        "data",
    ] {
        assert!(save(dir.path(), "test", &before, &123));
        if change == "malformed" {
            fs::write(&path, b"{broken").unwrap();
        } else {
            let connection = rusqlite::Connection::open(&path).unwrap();
            let section = if matches!(change, "schema_version" | "journal_schema_version") {
                "source"
            } else {
                "data"
            };
            if change == "checksum" {
                connection
                    .execute("UPDATE records SET checksum=x'00' WHERE section='data'", [])
                    .unwrap();
            } else if section == "source" {
                let bytes: Vec<u8> = connection
                    .query_row(
                        "SELECT payload FROM records WHERE section='source'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                let mut source: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                source[if change == "schema_version" {
                    "format"
                } else {
                    "journal_schema"
                }] = if change == "schema_version" { 1 } else { 0 }.into();
                let bytes = serde_json::to_vec(&source).unwrap();
                connection
                    .execute(
                        "UPDATE records SET payload=?1,checksum=?2 WHERE section='source'",
                        rusqlite::params![&bytes, digest(&SHA256, &bytes).as_ref()],
                    )
                    .unwrap();
            } else {
                connection
                    .execute("UPDATE records SET payload=x'30' WHERE section='data'", [])
                    .unwrap();
            }
        }
        assert!(load::<i32>(dir.path(), "test").is_none(), "{change}");
        assert!(load_any::<i32>(dir.path(), "test").is_none(), "{change}");
        assert_eq!(fingerprint(dir.path()), before);
    }
    // Reject over-limit files before allocating their contents.
    fs::File::create(&path)
        .unwrap()
        .set_len(MAX_BYTES as u64 + 1)
        .unwrap();
    assert!(load::<i32>(dir.path(), "test").is_none());
}

#[test]
fn failed_or_concurrent_save_never_installs_candidate() {
    let dir = source();
    let before = fingerprint(dir.path());
    assert!(!save(dir.path(), "../escape", &before, &123));
    struct Changing<'a>(&'a Path);
    impl Serialize for Changing<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            fs::write(self.0.join("journal.jsonl"), b"changed journal\n").unwrap();
            let mut state = serializer.serialize_struct("Changing", 1)?;
            state.serialize_field("value", &123)?;
            state.end()
        }
    }
    assert!(!save(dir.path(), "test", &before, &Changing(dir.path())));
    assert!(!cache_path(dir.path(), "test").unwrap().exists());
    assert!(fs::read_dir(dir.path()).unwrap().all(|entry| {
        let name = entry.unwrap().file_name();
        !name.as_bytes().starts_with(b".read-") || name.as_bytes().ends_with(b".lock")
    }));
    let current = fingerprint(dir.path());
    fs::create_dir(cache_path(dir.path(), "test").unwrap()).unwrap();
    assert!(!save(dir.path(), "test", &current, &123));
    assert_eq!(fingerprint(dir.path()), current);
    assert!(fs::read_dir(dir.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .as_bytes()
            .windows(5)
            .any(|s| s == b".tmp.")
    }));
}

#[test]
fn fingerprint_rejects_nonregular_sources_and_respects_blob_limit() {
    let dir = source();
    assert!(SourceFingerprint::read(dir.path(), 0).is_none());
    assert!(SourceFingerprint::read(dir.path(), 1).is_some());
    fs::write(dir.path().join("blobs/second.json"), b"second").unwrap();
    assert!(SourceFingerprint::read(dir.path(), 1).is_none());
    std::os::unix::fs::symlink("payload.json", dir.path().join("blobs/link")).unwrap();
    assert!(SourceFingerprint::read(dir.path(), MAX_BLOBS).is_none());
    fs::remove_file(dir.path().join("blobs/link")).unwrap();
    fs::remove_file(dir.path().join("journal.jsonl")).unwrap();
    fs::create_dir(dir.path().join("journal.jsonl")).unwrap();
    assert!(SourceFingerprint::read(dir.path(), MAX_BLOBS).is_none());
}

#[test]
fn stale_snapshot_requires_current_source_and_cannot_hide_damaged_cache() {
    let dir = source();
    let before = fingerprint(dir.path());
    assert!(save(dir.path(), "test", &before, &vec![1, 2]));
    assert!(load_any::<Vec<i32>>(dir.path(), "test").unwrap().2);
    fs::write(dir.path().join("journal.jsonl"), b"new version\n").unwrap();
    assert!(load::<Vec<i32>>(dir.path(), "test").is_none());
    let (cached_source, data, fresh) = load_any::<Vec<i32>>(dir.path(), "test").unwrap();
    assert_eq!(cached_source, before);
    assert_eq!(data, vec![1, 2]);
    assert!(!fresh);
    // A fresh cache saved by the authoritative reader is visible immediately.
    let current = fingerprint(dir.path());
    assert!(save(dir.path(), "test", &current, &vec![1, 2, 3]));
    assert_eq!(
        load::<Vec<i32>>(dir.path(), "test").unwrap().1,
        vec![1, 2, 3]
    );
    fs::remove_file(dir.path().join("journal.jsonl")).unwrap();
    fs::create_dir(dir.path().join("journal.jsonl")).unwrap();
    assert!(load_any::<Vec<i32>>(dir.path(), "test").is_none());
    fs::remove_dir(dir.path().join("journal.jsonl")).unwrap();
    fs::write(dir.path().join("journal.jsonl"), b"newer version\n").unwrap();
    let path = cache_path(dir.path(), "test").unwrap();
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute(
            "UPDATE records SET payload=?1 WHERE section='data'",
            [b"[99]".as_slice()],
        )
        .unwrap();
    drop(connection);
    assert!(load_any::<Vec<i32>>(dir.path(), "test").is_none());
}

#[test]
fn changing_current_source_during_decode_keeps_valid_cache_as_stale() {
    #[derive(Serialize)]
    struct Changing {
        journal: PathBuf,
        value: u32,
    }
    impl<'de> Deserialize<'de> for Changing {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            #[derive(Deserialize)]
            struct Fields {
                journal: PathBuf,
                value: u32,
            }
            let fields = Fields::deserialize(deserializer)?;
            fs::write(&fields.journal, b"changed during cache decoding\n")
                .map_err(serde::de::Error::custom)?;
            Ok(Self {
                journal: fields.journal,
                value: fields.value,
            })
        }
    }
    let dir = source();
    let before = fingerprint(dir.path());
    let data = Changing {
        journal: dir.path().join("journal.jsonl"),
        value: 123,
    };
    assert!(save(dir.path(), "test", &before, &data));
    let (cached_source, data, fresh) = load_any::<Changing>(dir.path(), "test").unwrap();
    assert_eq!(cached_source, before);
    assert_eq!(data.value, 123);
    assert!(
        !fresh,
        "a mid-decode append must not label the view current"
    );
    assert_ne!(fingerprint(dir.path()), before);
}

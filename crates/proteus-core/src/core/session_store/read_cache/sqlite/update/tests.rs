use super::*;

fn source() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("session.json"), b"metadata").unwrap();
    fs::write(dir.path().join("journal.jsonl"), b"journal\n").unwrap();
    dir
}

#[test]
fn unchanged_payload_is_not_rewritten_and_damaged_payload_is_healed() {
    let dir = source();
    let source = SourceFingerprint::read(dir.path(), MAX_BLOBS).unwrap();
    let payload = "synthetic payload ".repeat(10_000);
    let writer = Writer::new(dir.path(), "test", &source).unwrap();
    writer.one("data", &payload).unwrap();
    writer.finish().unwrap();
    let path = cache_path(dir.path(), "test").unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE changes(section TEXT);
            CREATE TRIGGER changed AFTER UPDATE ON records
            BEGIN INSERT INTO changes VALUES(NEW.section); END;",
        )
        .unwrap();
    drop(connection);

    let writer = Updater::new(dir.path(), "test", &source).unwrap();
    assert!(writer.row("data", 0, &payload).unwrap());
    // Replacements and other in-place writers use the same cross-process lock.
    assert!(Writer::new(dir.path(), "test", &source).is_none());
    assert!(Updater::new(dir.path(), "test", &source).is_none());
    writer.finish().unwrap();
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM changes", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    // The expected checksum alone is insufficient: damaged bytes must not be
    // retained merely because the canonical payload still has the same hash.
    connection
        .execute("UPDATE records SET payload=x'00' WHERE section='data'", [])
        .unwrap();
    drop(connection);
    let writer = Updater::new(dir.path(), "test", &source).unwrap();
    assert!(!writer.row("data", 0, &payload).unwrap());
    writer.finish().unwrap();
    assert_eq!(
        Reader::open(dir.path(), "test")
            .unwrap()
            .one::<String>("data")
            .unwrap(),
        payload
    );
}

#[test]
fn changed_source_rolls_back_updates_and_releases_cache_lock() {
    let dir = source();
    let source = SourceFingerprint::read(dir.path(), MAX_BLOBS).unwrap();
    let writer = Writer::new(dir.path(), "test", &source).unwrap();
    writer.one("data", &"before").unwrap();
    writer.finish().unwrap();
    let writer = Updater::new(dir.path(), "test", &source).unwrap();
    assert!(!writer.row("data", 0, &"uncommitted").unwrap());
    fs::write(dir.path().join("journal.jsonl"), b"new journal\n").unwrap();
    assert!(writer.finish().is_none());
    let reader = Reader::open(dir.path(), "test").unwrap();
    assert_eq!(reader.one::<String>("data").unwrap(), "before");
    assert!(!reader.finish().unwrap().1);
    let current = SourceFingerprint::read(dir.path(), MAX_BLOBS).unwrap();
    assert!(Updater::new(dir.path(), "test", &current).is_some());
}

#[test]
fn replacement_gates_new_opens_and_keeps_the_previous_snapshot_readable() {
    let dir = source();
    let source = SourceFingerprint::read(dir.path(), MAX_BLOBS).unwrap();
    let writer = Writer::new(dir.path(), "test", &source).unwrap();
    writer.one("data", &"old").unwrap();
    writer.finish().unwrap();
    let old = Reader::open(dir.path(), "test").unwrap();

    let writer = Writer::new(dir.path(), "test", &source).unwrap();
    // A new reader cannot cross the hot-journal cleanup/rename window.
    assert!(Reader::open(dir.path(), "test").is_none());
    assert_eq!(
        writer
            .previous("test")
            .unwrap()
            .one::<String>("data")
            .unwrap(),
        "old"
    );
    writer.one("data", &"new").unwrap();
    writer.finish().unwrap();
    assert_eq!(old.one::<String>("data").unwrap(), "old");
    assert_eq!(
        Reader::open(dir.path(), "test")
            .unwrap()
            .one::<String>("data")
            .unwrap(),
        "new"
    );
}

#[test]
fn reader_opens_during_update_and_reading_never_creates_a_missing_gate() {
    let dir = source();
    let source = SourceFingerprint::read(dir.path(), MAX_BLOBS).unwrap();
    let writer = Writer::new(dir.path(), "test", &source).unwrap();
    writer.one("data", &"old").unwrap();
    writer.finish().unwrap();
    let writer = Updater::new(dir.path(), "test", &source).unwrap();
    writer.one("data", &"new").unwrap();
    let reader = Reader::open(dir.path(), "test").unwrap();
    assert_eq!(reader.one::<String>("data").unwrap(), "old");
    drop(reader);
    writer.finish().unwrap();
    assert_eq!(
        Reader::open(dir.path(), "test")
            .unwrap()
            .one::<String>("data")
            .unwrap(),
        "new"
    );

    let gate = dir.path().join(".read-test.lock");
    fs::remove_file(&gate).unwrap();
    assert!(Reader::open(dir.path(), "test").is_none());
    assert!(
        !gate.exists(),
        "read-only cache lookup must not create files"
    );
}

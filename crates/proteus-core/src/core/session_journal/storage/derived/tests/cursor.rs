use super::*;
use crate::core::session_journal::JournalReadCursor;

fn history_fixture() -> Fixture {
    let mut fixture = Fixture::new();
    for text in ["first", "later", "third"] {
        fixture.history(
            HistoryMutationKind::Append,
            vec![CanonicalMessage::text(
                MessageRole::User,
                text.repeat(20_000),
            )],
            false,
            false,
        );
    }
    fixture
}

fn changed_cursor(
    cursor: &JournalReadCursor,
    key: &str,
    value: serde_json::Value,
) -> JournalReadCursor {
    let mut encoded = serde_json::to_value(cursor).unwrap();
    encoded[key] = value;
    serde_json::from_value(encoded).unwrap()
}

#[test]
fn cursors_match_every_complete_prefix_and_both_encodings() {
    let fixture = history_fixture();
    for blobs in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut records: Vec<_> = fixture.records.iter().map(stored).collect();
        if blobs {
            for record in &mut records {
                let StoredPayload::Inline { value } = &record.payload else {
                    unreachable!()
                };
                record.payload =
                    write_blob(dir.path(), &serde_json::to_vec(value).unwrap()).unwrap();
            }
        }
        for end in 0..=records.len() {
            write_stored(dir.path(), &records[..end], b"{torn tail");
            let read = ValidatedJournalRead::open(dir.path(), fixture.session).unwrap();
            let cursor = read.cursor();
            let expected_fingerprints: Vec<_> = read
                .history()
                .iter()
                .map(history_message_sha256)
                .collect::<Result<_>>()
                .unwrap();
            assert_eq!(cursor.last_session_seq(), end as u64);
            let bytes = std::fs::read(journal_path(dir.path())).unwrap();
            assert_eq!(
                cursor.complete_len() as usize,
                bytes.len() - b"{torn tail".len()
            );
            let roundtrip: JournalReadCursor =
                serde_json::from_value(serde_json::to_value(&cursor).unwrap()).unwrap();
            assert_eq!(roundtrip, cursor);
            write_stored(dir.path(), &records, b"{another torn tail");
            let (mut read, matches) =
                ValidatedJournalRead::open_with_cursor(dir.path(), fixture.session, Some(&cursor))
                    .unwrap();
            assert!(matches);
            assert_eq!(
                read.take_prefix_history_fingerprints(),
                Some(expected_fingerprints)
            );
            assert_eq!(read.take_prefix_history_fingerprints(), None);
            let (mut snapshot, _) = read.into_parts();
            let mut suffix = Vec::new();
            snapshot
                .scan_selected_after(
                    &cursor,
                    |_| true,
                    |record| {
                        suffix.push(record);
                        Ok(())
                    },
                )
                .unwrap();
            assert_eq!(suffix, fixture.records[end..]);
            let mut selected = Vec::new();
            snapshot
                .scan_selected_after(
                    &cursor,
                    |_| false,
                    |record| {
                        selected.push(record);
                        Ok(())
                    },
                )
                .unwrap();
            assert!(selected.is_empty());
        }
    }
}

#[test]
fn cursor_candidates_are_hints_and_invalid_identity_is_a_cache_miss() {
    let fixture = history_fixture();
    let dir = tempfile::tempdir().unwrap();
    let records: Vec<_> = fixture.records.iter().map(stored).collect();
    write_stored(dir.path(), &records[..2], b"");
    let cursor = ValidatedJournalRead::open(dir.path(), fixture.session)
        .unwrap()
        .cursor();
    write_stored(dir.path(), &records, b"");
    let candidates = [
        changed_cursor(
            &cursor,
            "session_id",
            serde_json::to_value(new_session_id()).unwrap(),
        ),
        changed_cursor(&cursor, "complete_len", (cursor.complete_len() - 1).into()),
        changed_cursor(&cursor, "complete_len", u64::MAX.into()),
        changed_cursor(&cursor, "last_session_seq", 1.into()),
        changed_cursor(
            &cursor,
            "prefix_sha256",
            serde_json::to_value([0_u8; 32]).unwrap(),
        ),
    ];
    for candidate in candidates {
        let (mut read, matches) =
            ValidatedJournalRead::open_with_cursor(dir.path(), fixture.session, Some(&candidate))
                .unwrap();
        assert!(!matches);
        assert_eq!(read.take_prefix_history_fingerprints(), None);
        assert_eq!(read.history().len(), 3);
        let (mut snapshot, _) = read.into_parts();
        assert!(
            snapshot
                .scan_selected_after(&candidate, |_| true, |_| Ok(()))
                .is_err()
        );
    }
    // A fresh inode with an identical prefix remains a valid cache source.
    let original = std::fs::read(journal_path(dir.path())).unwrap();
    std::fs::rename(journal_path(dir.path()), dir.path().join("old-journal")).unwrap();
    std::fs::write(journal_path(dir.path()), &original).unwrap();
    assert!(
        ValidatedJournalRead::open_with_cursor(dir.path(), fixture.session, Some(&cursor))
            .unwrap()
            .1
    );
    // Same length does not establish identity, nor does truncation.
    let changed = String::from_utf8(original)
        .unwrap()
        .replace("first", "other");
    std::fs::write(journal_path(dir.path()), changed).unwrap();
    assert!(
        !ValidatedJournalRead::open_with_cursor(dir.path(), fixture.session, Some(&cursor))
            .unwrap()
            .1
    );
    write_stored(dir.path(), &records[..1], b"");
    assert!(
        !ValidatedJournalRead::open_with_cursor(dir.path(), fixture.session, Some(&cursor))
            .unwrap()
            .1
    );
}

#[test]
fn suffix_pass_requires_validation_and_rejects_snapshot_mutation() {
    let fixture = history_fixture();
    let dir = tempfile::tempdir().unwrap();
    let records: Vec<_> = fixture.records.iter().map(stored).collect();
    write_stored(dir.path(), &records[..1], b"");
    let cursor = ValidatedJournalRead::open(dir.path(), fixture.session)
        .unwrap()
        .cursor();
    write_stored(dir.path(), &records, b"");
    let mut unvalidated = read::JournalReadSnapshot::open(dir.path(), fixture.session).unwrap();
    assert!(
        unvalidated
            .scan_selected_after(&cursor, |_| true, |_| Ok(()))
            .unwrap_err()
            .to_string()
            .contains("requires a complete typed scan")
    );
    let (read, matches) =
        ValidatedJournalRead::open_with_cursor(dir.path(), fixture.session, Some(&cursor)).unwrap();
    assert!(matches);
    let (mut snapshot, _) = read.into_parts();
    let original = std::fs::read_to_string(journal_path(dir.path())).unwrap();
    // Mutation in the skipped prefix is detected before emitting suffix rows.
    std::fs::write(journal_path(dir.path()), original.replace("first", "other")).unwrap();
    assert!(
        snapshot
            .scan_selected_after(&cursor, |_| true, |_| panic!("changed prefix"))
            .is_err()
    );
    // Mutation in the suffix is rejected before successful publication.
    std::fs::write(journal_path(dir.path()), original.replace("later", "other")).unwrap();
    assert!(
        snapshot
            .scan_selected_after(&cursor, |_| true, |_| Ok(()))
            .unwrap_err()
            .to_string()
            .contains("complete prefix changed")
    );
    let mut malformed: Vec<_> = original.lines().collect();
    malformed[2] = "invalid JSON";
    std::fs::write(
        journal_path(dir.path()),
        format!("{}\n", malformed.join("\n")),
    )
    .unwrap();
    let error = snapshot
        .scan_selected_after(&cursor, |_| true, |_| Ok(()))
        .unwrap_err();
    assert!(format!("{error:#}").contains("line 3"));
    std::fs::write(
        journal_path(dir.path()),
        &original.as_bytes()[..cursor.complete_len() as usize - 1],
    )
    .unwrap();
    let error = snapshot
        .scan_selected_after(&cursor, |_| true, |_| Ok(()))
        .unwrap_err();
    assert!(format!("{error:#}").contains("shortened during snapshot read"));
    std::fs::write(journal_path(dir.path()), &original).unwrap();
    snapshot
        .scan_selected_after(&cursor, |_| true, |_| Ok(()))
        .unwrap();
    // Later path replacement and appends cannot change the captured inode/prefix.
    std::fs::rename(journal_path(dir.path()), dir.path().join("old-journal")).unwrap();
    std::fs::write(journal_path(dir.path()), b"invalid replacement\n").unwrap();
    snapshot
        .scan_selected_after(&cursor, |_| true, |_| Ok(()))
        .unwrap();
}

#[test]
fn candidate_cursor_never_hides_old_blob_corruption_or_deletion() {
    let fixture = history_fixture();
    let dir = tempfile::tempdir().unwrap();
    let mut records: Vec<_> = fixture.records.iter().map(stored).collect();
    let payload = serde_json::to_vec(&fixture.records[0].entry.payload_value().unwrap()).unwrap();
    records[0].payload = write_blob(dir.path(), &payload).unwrap();
    let StoredPayload::Blob { relative_path, .. } = &records[0].payload else {
        unreachable!()
    };
    let blob = dir.path().join(relative_path);
    write_stored(dir.path(), &records[..1], b"");
    let cursor = ValidatedJournalRead::open(dir.path(), fixture.session)
        .unwrap()
        .cursor();
    write_stored(dir.path(), &records, b"");
    let mut corrupt = payload.clone();
    corrupt[0] ^= 1;
    std::fs::write(&blob, &corrupt).unwrap();
    let error = ValidatedJournalRead::open_with_cursor(dir.path(), fixture.session, Some(&cursor))
        .unwrap_err();
    assert!(format!("{error:#}").contains("hash mismatch"));
    std::fs::remove_file(&blob).unwrap();
    let error = ValidatedJournalRead::open_with_cursor(dir.path(), fixture.session, Some(&cursor))
        .unwrap_err();
    assert!(format!("{error:#}").contains("failed to inspect journal blob"));
}

#[test]
fn candidate_does_not_change_late_storage_error_precedence() {
    let fixture = history_fixture();
    let dir = tempfile::tempdir().unwrap();
    let mut records: Vec<_> = fixture.records.iter().map(stored).collect();
    let mut cursors = Vec::new();
    for end in 0..=records.len() {
        write_stored(dir.path(), &records[..end], b"");
        cursors.push(
            ValidatedJournalRead::open(dir.path(), fixture.session)
                .unwrap()
                .cursor(),
        );
    }
    records[1].session_seq = 99;
    // Earlier semantic sequence error must not mask a later JSON decode error.
    write_stored(dir.path(), &records[..2], b"invalid JSON\n");
    let expected = format!(
        "{:#}",
        ValidatedJournalRead::open(dir.path(), fixture.session).unwrap_err()
    );
    assert!(expected.contains("line 3"));
    for previous in std::iter::once(None).chain(cursors.iter().map(Some)) {
        let actual = ValidatedJournalRead::open_with_cursor(dir.path(), fixture.session, previous)
            .unwrap_err();
        assert_eq!(format!("{actual:#}"), expected);
    }
}

#[test]
fn empty_cursor_handles_missing_snapshot_and_ignores_later_path_creation() {
    let dir = tempfile::tempdir().unwrap();
    let session = new_session_id();
    let read = ValidatedJournalRead::open(dir.path(), session).unwrap();
    let cursor = read.cursor();
    assert_eq!(cursor.complete_len(), 0);
    assert_eq!(cursor.last_session_seq(), 0);
    assert!(
        ValidatedJournalRead::open_with_cursor(dir.path(), session, Some(&cursor))
            .unwrap()
            .1
    );
    let (mut snapshot, _) = read.into_parts();
    std::fs::write(journal_path(dir.path()), b"invalid later file\n").unwrap();
    snapshot
        .scan_selected_after(&cursor, |_| true, |_| panic!("missing snapshot is fixed"))
        .unwrap();
}

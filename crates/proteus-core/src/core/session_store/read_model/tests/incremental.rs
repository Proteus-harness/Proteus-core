use super::*;
use crate::app_server::journal_transcript_append_from_source;

pub(super) fn oracle(store: &SessionStore) -> SessionReadModel {
    SessionReadModel::from_validated(
        store,
        ValidatedJournalRead::open(store.session_dir(), store.session_id()).unwrap(),
        render,
    )
    .unwrap()
}

pub(super) fn project(store: &SessionStore) -> SessionReadView {
    store
        .project_read_view(render, journal_transcript_append_from_source)
        .unwrap()
}

async fn append_user(store: &SessionStore, text: &str) {
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, text)],
        )
        .await
        .unwrap();
}

fn checkpoint_bytes(store: &SessionStore) -> Vec<u8> {
    rusqlite::Connection::open(store.session_dir().join(".read-view.sqlite"))
        .unwrap()
        .query_row(
            "SELECT payload FROM records WHERE section='projection' AND position=0",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[tokio::test]
async fn incremental_append_matches_full_oracle_and_touches_only_new_rows() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "first").await;
    let old = project(&store);
    let old_revision = old.cursor.unwrap().revision;
    let old_checkpoint = checkpoint_bytes(&store);
    let db = rusqlite::Connection::open(store.session_dir().join(".read-view.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TABLE changes(section TEXT,position INTEGER,kind TEXT);
        CREATE TRIGGER added AFTER INSERT ON records
        BEGIN INSERT INTO changes VALUES(NEW.section,NEW.position,'insert'); END;
        CREATE TRIGGER changed AFTER UPDATE ON records
        BEGIN INSERT INTO changes VALUES(NEW.section,NEW.position,'update'); END;",
    )
    .unwrap();
    drop(db);
    add_exchange(&store, workspace.path()).await;
    let expected = oracle(&store);
    let journal = fs::read(store.journal_path()).unwrap();
    let appended = Cell::new(false);
    let view = store
        .project_read_view(
            |_| panic!("an independent settled turn must use its suffix"),
            |source, cursor, baseline, final_len| {
                appended.set(true);
                journal_transcript_append_from_source(source, cursor, baseline, final_len)
            },
        )
        .unwrap();
    assert!(appended.get());
    assert_display_view(&view, &expected);
    assert_same_view(&store.fresh_read_model().unwrap(), &expected);
    let base = view.cursor.unwrap().base.unwrap();
    assert_eq!(base.revision, old_revision);
    assert_eq!(base.common_prefix, 1);
    assert_ne!(checkpoint_bytes(&store), old_checkpoint);
    let db = rusqlite::Connection::open(store.session_dir().join(".read-view.sqlite")).unwrap();
    let old_changed: i64 = db
        .query_row(
            "SELECT count(*) FROM changes WHERE section IN ('conversation','transcript') AND position=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_changed, 0, "old text rows stay untouched");
    assert_eq!(fs::read(store.journal_path()).unwrap(), journal);
}

#[tokio::test]
async fn incremental_append_reads_only_requested_old_display_rows() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path(), workspace.path(), new_session_id()).unwrap();
    let messages: Vec<_> = (0..75)
        .map(|index| CanonicalMessage::text(MessageRole::User, format!("message {index}")))
        .collect();
    store
        .append_history(new_thread_id(), None, &messages)
        .await
        .unwrap();
    project(&store);
    let db = rusqlite::Connection::open(store.session_dir().join(".read-view.sqlite")).unwrap();
    db.execute(
        "UPDATE records SET payload=x'30' WHERE section='conversation' OR (section='transcript' AND position < 25)",
        [],
    )
    .unwrap();
    drop(db);
    append_user(&store, "independent tail").await;
    let expected = oracle(&store);
    let view = store
        .project_read_view(
            |_| panic!("unrequested cached rows must not force a full decode"),
            journal_transcript_append_from_source,
        )
        .unwrap();
    assert_display_view(&view, &expected);
    assert!(store.saved_transcript_page(Some(25), None).is_none());
    assert!(store.fresh_read_model().is_none());
}

#[tokio::test]
async fn damaged_requested_tail_falls_back_and_heals_the_full_projection() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "first").await;
    project(&store);
    let db = rusqlite::Connection::open(store.session_dir().join(".read-view.sqlite")).unwrap();
    db.execute(
        "UPDATE records SET payload=x'30' WHERE section='transcript'",
        [],
    )
    .unwrap();
    drop(db);
    append_user(&store, "tail").await;
    let rendered = Cell::new(false);
    let view = store
        .project_read_view(
            |source| {
                rendered.set(true);
                render(source)
            },
            journal_transcript_append_from_source,
        )
        .unwrap();
    assert!(rendered.get());
    let expected = oracle(&store);
    assert_display_view(&view, &expected);
    assert_same_view(&store.fresh_read_model().unwrap(), &expected);
}

#[tokio::test]
async fn full_oracle_save_invalidates_previous_projection_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "first").await;
    project(&store);
    store.project_read_model(render).unwrap();
    let db = rusqlite::Connection::open(store.session_dir().join(".read-view.sqlite")).unwrap();
    let checkpoints: i64 = db
        .query_row(
            "SELECT count(*) FROM records WHERE section='projection'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(checkpoints, 0);
    drop(db);
    append_user(&store, "tail").await;
    let rendered = Cell::new(0);
    let view = store
        .project_read_view(
            |source| {
                rendered.set(rendered.get() + 1);
                render(source)
            },
            |_, _, _, _| panic!("a full writer without a cursor must invalidate the checkpoint"),
        )
        .unwrap();
    assert_eq!(rendered.get(), 1);
    assert_display_view(&view, &oracle(&store));
}

#[tokio::test]
async fn damaged_missing_or_incompatible_checkpoint_rebuilds_full_view() {
    for damage in ["missing", "checksum", "version", "header"] {
        let root = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let store = saved(root.path(), workspace.path(), "first").await;
        project(&store);
        let db = rusqlite::Connection::open(store.session_dir().join(".read-view.sqlite")).unwrap();
        match damage {
            "missing" => {
                db.execute("DELETE FROM records WHERE section='projection'", [])
                    .unwrap();
            }
            "checksum" => {
                db.execute(
                    "UPDATE records SET payload=x'30' WHERE section='projection'",
                    [],
                )
                .unwrap();
            }
            "version" | "header" => {
                let mut value: serde_json::Value =
                    serde_json::from_slice(&checkpoint_bytes(&store)).unwrap();
                if damage == "version" {
                    value["format"] = 999.into();
                } else {
                    value["header_sha256"][0] =
                        (value["header_sha256"][0].as_u64().unwrap() ^ 1).into();
                }
                let bytes = serde_json::to_vec(&value).unwrap();
                let hash = ring::digest::digest(&ring::digest::SHA256, &bytes);
                db.execute(
                    "UPDATE records SET payload=?1,checksum=?2 WHERE section='projection'",
                    rusqlite::params![bytes, hash.as_ref()],
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        drop(db);
        append_user(&store, "tail").await;
        let rendered = Cell::new(false);
        let view = store
            .project_read_view(
                |source| {
                    rendered.set(true);
                    render(source)
                },
                |_, _, _, _| panic!("{damage} checkpoint must not authorize incremental rendering"),
            )
            .unwrap();
        assert!(rendered.get(), "{damage}");
        assert_display_view(&view, &oracle(&store));
        assert!(sqlite::checkpoint(&store).is_some());
    }
}

#[tokio::test]
async fn unsupported_suffix_uses_full_renderer_from_the_validated_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "first").await;
    project(&store);
    store
        .replace_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, "replaced")],
            None,
        )
        .await
        .unwrap();
    let rendered = Cell::new(false);
    let view = store
        .project_read_view(
            |source| {
                rendered.set(true);
                render(source)
            },
            journal_transcript_append_from_source,
        )
        .unwrap();
    assert!(rendered.get());
    assert_display_view(&view, &oracle(&store));
}

#[tokio::test]
async fn failed_append_transaction_rolls_back_rows_before_full_fallback() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "first").await;
    project(&store);
    let before = checkpoint_bytes(&store);
    let path = store.session_dir().join(".read-view.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER reject_header BEFORE UPDATE ON records WHEN NEW.section='header'
        BEGIN SELECT RAISE(ABORT,'synthetic cache failure'); END;",
    )
    .unwrap();
    drop(db);
    append_user(&store, "tail").await;
    let rendered = Cell::new(false);
    let view = store
        .project_read_view(
            |source| {
                rendered.set(true);
                assert_eq!(checkpoint_bytes(&store), before);
                let db = rusqlite::Connection::open(&path).unwrap();
                let rows: i64 = db.query_row(
                    "SELECT count(*) FROM records WHERE section IN ('conversation','transcript')",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(rows, 2, "suffix rows must roll back before fallback");
                render(source)
            },
            journal_transcript_append_from_source,
        )
        .unwrap();
    assert!(rendered.get());
    let expected = oracle(&store);
    assert_display_view(&view, &expected);
    assert_same_view(&store.fresh_read_model().unwrap(), &expected);
}

#[tokio::test]
async fn cache_contention_returns_verified_owned_view_without_publishing() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "first").await;
    project(&store);
    append_user(&store, "tail").await;
    let expected = oracle(&store);
    let gate = std::cell::RefCell::new(None);
    let rendered = Cell::new(false);
    let view = store
        .project_read_view(
            |source| {
                rendered.set(true);
                render(source)
            },
            |source, cursor, baseline, final_len| {
                let lock = fs::File::open(store.session_dir().join(".read-view.lock"))?;
                lock.try_lock()?;
                *gate.borrow_mut() = Some(lock);
                journal_transcript_append_from_source(source, cursor, baseline, final_len)
            },
        )
        .unwrap();
    assert!(rendered.get());
    assert!(view.cursor.is_none());
    assert_eq!(view.summary, expected.summary);
    assert_eq!(view.revision, expected.conversation.revision);
    assert_eq!(view.usage, expected.usage);
    assert_eq!(
        serde_json::to_value(view.transcript).unwrap(),
        serde_json::to_value(expected.transcript).unwrap()
    );
    drop(gate);
    assert!(store.fresh_read_model().is_none());
}

#[tokio::test]
async fn concurrent_full_rebuild_cannot_rebind_a_suffix_to_its_rows() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "first").await;
    project(&store);
    append_user(&store, "tail").await;
    let rendered = Cell::new(false);
    let view = store
        .project_read_view(
            |source| {
                rendered.set(true);
                render(source)
            },
            |source, cursor, baseline, final_len| {
                let suffix =
                    journal_transcript_append_from_source(source, cursor, baseline, final_len)?;
                store.project_read_model(render)?;
                Ok(suffix)
            },
        )
        .unwrap();
    assert!(rendered.get());
    assert_display_view(&view, &oracle(&store));
}

#[tokio::test]
async fn source_metadata_race_keeps_the_verified_result_but_refuses_publication() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "first").await;
    project(&store);
    append_user(&store, "tail").await;
    let journal = fs::read(store.journal_path()).unwrap();
    let rendered = Cell::new(false);
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let view = store
        .project_read_view(
            |source| {
                rendered.set(true);
                render(source)
            },
            |source, cursor, baseline, final_len| {
                let suffix =
                    journal_transcript_append_from_source(source, cursor, baseline, final_len)?;
                fs::File::open(store.journal_path())?
                    .set_times(fs::FileTimes::new().set_modified(modified))?;
                Ok(suffix)
            },
        )
        .unwrap();
    assert!(rendered.get());
    assert!(view.cursor.is_none());
    assert_eq!(view.summary, oracle(&store).summary);
    assert_eq!(fs::read(store.journal_path()).unwrap(), journal);
    assert!(store.fresh_read_model().is_none());
}

#[tokio::test]
async fn incremental_checkpoint_never_skips_old_blob_validation() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), &"old blob ".repeat(40_000)).await;
    project(&store);
    append_user(&store, "tail").await;
    let journal = fs::read(store.journal_path()).unwrap();
    let blob = fs::read_dir(store.session_dir().join("blobs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut damaged = fs::read(&blob).unwrap();
    damaged[0] ^= 1;
    fs::write(&blob, &damaged).unwrap();
    assert!(
        store
            .project_read_view(
                |_| panic!("invalid old blobs must fail before full render"),
                |_, _, _, _| panic!("a checkpoint cannot bypass old blob validation"),
            )
            .is_err()
    );
    assert_eq!(fs::read(store.journal_path()).unwrap(), journal);
    assert_eq!(fs::read(blob).unwrap(), damaged);
}

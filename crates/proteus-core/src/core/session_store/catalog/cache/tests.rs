use super::*;
use crate::{
    core::SessionStore,
    domain::{new_session_id, new_thread_id},
    model_standard::{CanonicalMessage, MessageRole},
};
use std::{cell::Cell, fs, time::Instant};

async fn saved(root: &Path, workspace: &Path, text: &str) -> SessionStore {
    let store = SessionStore::new(root, workspace, new_session_id()).unwrap();
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, text)],
        )
        .await
        .unwrap();
    store
}
fn counted(
    cache: &Mutex<SummaryCache>,
    dir: &Path,
    reads: &Cell<usize>,
) -> Result<AppSessionSummary> {
    load(cache, dir.to_owned(), |dir| {
        reads.set(reads.get() + 1);
        super::super::session_summary_from_dir(dir)
    })
}

fn refresh(
    cache: &Mutex<SummaryCache>,
    dir: &Path,
    reads: &Cell<usize>,
) -> Result<AppSessionSummary> {
    let before = SourceFingerprint::read(dir, MAX_DISK_BLOBS);
    reads.set(reads.get() + 1);
    let summary = super::super::session_summary_from_dir(dir.to_owned())?;
    cache_if_unchanged(cache, dir, before, summary.clone());
    Ok(summary)
}

#[tokio::test]
async fn stale_catalog_is_immediate_then_authoritative_refresh_becomes_visible() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "first").await;
    let cache = Mutex::new(SummaryCache::default());
    let reads = Cell::new(0);
    let first = counted(&cache, store.session_dir(), &reads).unwrap();
    for _ in 0..5 {
        assert_eq!(counted(&cache, store.session_dir(), &reads).unwrap(), first);
    }
    assert_eq!(reads.get(), 1);
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::Assistant, "reply")],
        )
        .await
        .unwrap();
    assert_eq!(counted(&cache, store.session_dir(), &reads).unwrap(), first);
    assert!(
        cache.lock().unwrap().0.is_empty(),
        "stale data must not enter memory cache"
    );
    assert_eq!(reads.get(), 1);
    let appended = refresh(&cache, store.session_dir(), &reads).unwrap();
    assert_eq!(appended.message_count, 2);
    assert_eq!(
        counted(&cache, store.session_dir(), &reads).unwrap(),
        appended
    );
    store
        .replace_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, "replacement")],
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        counted(&cache, store.session_dir(), &reads).unwrap(),
        appended
    );
    // A fresh summary saved by another reader must replace the stale placeholder.
    let independent = Mutex::new(SummaryCache::default());
    let replaced = refresh(&independent, store.session_dir(), &reads).unwrap();
    assert_eq!(replaced.message_count, 1);
    assert_eq!(replaced.preview.as_deref(), Some("replacement"));
    assert_eq!(
        counted(&cache, store.session_dir(), &reads).unwrap(),
        replaced
    );
    assert_eq!(reads.get(), 3);
    fs::remove_file(store.journal_path()).unwrap();
    assert_eq!(
        counted(&cache, store.session_dir(), &reads).unwrap(),
        replaced
    );
    refresh(&cache, store.session_dir(), &reads).unwrap();
    assert_eq!(
        counted(&cache, store.session_dir(), &reads)
            .unwrap()
            .message_count,
        0
    );
    assert_eq!(reads.get(), 4);
}

#[tokio::test]
async fn stale_display_is_allowed_but_identity_and_audit_stay_strict() {
    let workspace = tempfile::tempdir().unwrap();
    for target in ["metadata", "journal", "blob"] {
        let root = tempfile::tempdir().unwrap();
        let text = if target == "blob" {
            "large".repeat(60_000)
        } else {
            "small".into()
        };
        let store = saved(root.path(), workspace.path(), &text).await;
        let cache = Mutex::new(SummaryCache::default());
        let reads = Cell::new(0);
        let original = counted(&cache, store.session_dir(), &reads).unwrap();
        let path = match target {
            "metadata" => store.session_dir().join("session.json"),
            "journal" => store.journal_path(),
            _ => fs::read_dir(store.session_dir().join("blobs"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        };
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] = b'!';
        fs::write(&path, bytes).unwrap();
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let result = counted(&cache, store.session_dir(), &reads);
        if target == "metadata" {
            assert!(result.is_err(), "current identity must be valid");
            assert_eq!(reads.get(), 2);
        } else {
            assert_eq!(result.unwrap(), original);
            assert_eq!(
                reads.get(),
                1,
                "stale display should not replay the journal"
            );
        }
        assert!(super::super::list_session_summaries_for_audit(root.path(), None).is_err());
        assert!(refresh(&cache, store.session_dir(), &reads).is_err());
    }
}

#[tokio::test]
async fn concurrent_change_is_not_cached_and_cache_is_bounded() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "first").await;
    let cache = Mutex::new(SummaryCache::default());
    load(&cache, store.session_dir().to_owned(), |dir| {
        let result = super::super::session_summary_from_dir(dir)?;
        fs::write(store.journal_path(), b"{broken}\n")?;
        Ok(result)
    })
    .unwrap();
    assert!(cache.lock().unwrap().0.is_empty());
    assert!(counted(&cache, store.session_dir(), &Cell::new(0)).is_err());
    let valid = saved(root.path(), workspace.path(), "valid").await;
    let summary = super::super::session_summary_from_dir(valid.session_dir().to_owned()).unwrap();
    let mut cache = SummaryCache::default();
    for index in 0..MAX_SESSIONS + 10 {
        let mut item = summary.clone();
        item.session_dir = PathBuf::from(index.to_string());
        cache.insert(
            SourceFingerprint::read(valid.session_dir(), MAX_BLOBS).unwrap(),
            item,
        );
    }
    assert_eq!(cache.0.len(), MAX_SESSIONS);
    assert!(cache.take(Path::new("0")).is_none());
}

#[tokio::test]
async fn removed_workspace_is_not_served_from_cache() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "first").await;
    let cache = Mutex::new(SummaryCache::default());
    let reads = Cell::new(0);
    counted(&cache, store.session_dir(), &reads).unwrap();
    workspace.close().unwrap();
    assert!(counted(&cache, store.session_dir(), &reads).is_err());
}

#[tokio::test]
async fn prepared_resume_primes_catalog_but_never_uses_cached_validation() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "prepared").await;
    let prepared = super::super::PreparedSession::from_dir(store.session_dir().to_owned()).unwrap();
    let summary = load(shared_cache(), store.session_dir().to_owned(), |_| {
        panic!("prepared projection must prime the native summary cache")
    })
    .unwrap();
    assert_eq!(summary.preview.as_deref(), Some("prepared"));
    assert_eq!(prepared.message_count(), 1);
    // Even an unchanged warm catalog never substitutes for a startup projection.
    let second = super::super::PreparedSession::from_dir(store.session_dir().to_owned()).unwrap();
    assert_eq!(second.message_count(), 1);
    fs::write(store.journal_path(), b"{broken}\n").unwrap();
    assert!(super::super::PreparedSession::from_dir(store.session_dir().to_owned()).is_err());
    assert!(summary.message_count > 0);
}

#[tokio::test]
async fn prepared_read_changed_during_validation_does_not_prime_catalog() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "before").await;
    prime_from_read(store.session_dir(), || {
        let summary = super::super::session_summary_from_dir(store.session_dir().to_owned())?;
        fs::write(store.journal_path(), b"{broken}\n")?;
        Ok(((), summary))
    })
    .unwrap();
    let reads = Cell::new(0);
    assert!(counted(shared_cache(), store.session_dir(), &reads).is_err());
    assert_eq!(reads.get(), 1);
}

#[tokio::test]
#[ignore = "explicit storage measurement, no timing assertion"]
async fn catalog_benchmark() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let mut stores = Vec::new();
    let text = "catalog measurement ".repeat(100);
    for _ in 0..200 {
        let store = SessionStore::new(root.path(), workspace.path(), new_session_id()).unwrap();
        let messages = (0..40)
            .map(|_| CanonicalMessage::text(MessageRole::User, &text))
            .collect::<Vec<_>>();
        store
            .append_history(new_thread_id(), None, &messages)
            .await
            .unwrap();
        stores.push(store);
    }
    let cache = Mutex::new(SummaryCache::default());
    let reads = Cell::new(0);
    let baseline = Instant::now();
    let expected = stores
        .iter()
        .map(|s| super::super::session_summary_from_dir(s.session_dir().to_owned()).unwrap())
        .collect::<Vec<_>>();
    let baseline = baseline.elapsed();
    for store in &stores {
        counted(&cache, store.session_dir(), &reads).unwrap();
    }
    reads.set(0);
    let warm = Instant::now();
    let actual = stores
        .iter()
        .map(|s| counted(&cache, s.session_dir(), &reads).unwrap())
        .collect::<Vec<_>>();
    let warm = warm.elapsed();
    assert_eq!(actual, expected);
    assert_eq!(reads.get(), 0);
    stores[0]
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::Assistant, "new")],
        )
        .await
        .unwrap();
    let changed = Instant::now();
    refresh(&cache, stores[0].session_dir(), &reads).unwrap();
    for store in &stores {
        counted(&cache, store.session_dir(), &reads).unwrap();
    }
    assert_eq!(reads.get(), 1);
    println!(
        "CATALOG_BENCH sessions=200 messages=8000 full_read_ms={:.2} warm_ms={:.2} one_changed_ms={:.2} changed_full_reads={}",
        baseline.as_secs_f64() * 1000.,
        warm.as_secs_f64() * 1000.,
        changed.elapsed().as_secs_f64() * 1000.,
        reads.get()
    );
}

#[tokio::test]
async fn persisted_summary_survives_a_fresh_memory_cache_but_audit_remains_strict() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "persisted").await;
    let first = counted(
        &Mutex::new(SummaryCache::default()),
        store.session_dir(),
        &Cell::new(0),
    )
    .unwrap();
    let reloaded = load(
        &Mutex::new(SummaryCache::default()),
        store.session_dir().to_owned(),
        |_| panic!("unchanged persisted summary must survive a process restart"),
    )
    .unwrap();
    assert_eq!(reloaded, first);
    fs::write(store.journal_path(), b"{broken}\n").unwrap();
    assert!(super::super::list_session_summaries_for_audit(root.path(), None).is_err());
    assert_eq!(
        counted(
            &Mutex::new(SummaryCache::default()),
            store.session_dir(),
            &Cell::new(0),
        )
        .unwrap(),
        first
    );
}

#[tokio::test]
async fn persisted_summary_must_match_current_path_workspace_and_identity() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "canonical").await;
    let source = SourceFingerprint::read(store.session_dir(), MAX_BLOBS).unwrap();
    let expected = super::super::session_summary_from_dir(store.session_dir().to_owned()).unwrap();
    for field in ["session_dir", "workspace_path", "session_id"] {
        let mut wrong = expected.clone();
        match field {
            "session_dir" => wrong.session_dir = root.path().join("wrong"),
            "workspace_path" => wrong.workspace_path = root.path().join("wrong"),
            _ => wrong.session_id = new_session_id(),
        }
        assert!(read_cache::save(
            store.session_dir(),
            DISK_NAME,
            &source,
            &wrong
        ));
        let reads = Cell::new(0);
        let actual = counted(
            &Mutex::new(SummaryCache::default()),
            store.session_dir(),
            &reads,
        )
        .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(reads.get(), 1, "{field}");
    }
}

#[tokio::test]
async fn public_stale_summary_refreshes_unopened_session_and_notifies_subscribers() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store = saved(root.path(), workspace.path(), "unopened").await;
    let old = summary(store.session_dir().to_owned()).unwrap();
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(
                MessageRole::Assistant,
                "background reply",
            )],
        )
        .await
        .unwrap();
    let journal = fs::read(store.journal_path()).unwrap();
    let mut updates = subscribe_updates();
    // The public catalog returns the stale value before its queued work completes.
    let immediate = summary(store.session_dir().to_owned()).unwrap();
    assert_eq!(immediate, old);
    let current = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            updates.changed().await.unwrap();
            if let Some((_, current)) =
                read_cache::load::<AppSessionSummary>(store.session_dir(), DISK_NAME)
            {
                if current.message_count == 2 {
                    break current;
                }
            }
        }
    })
    .await
    .expect("queued catalog refresh should publish its fresh summary");
    assert_eq!(summary(store.session_dir().to_owned()).unwrap(), current);
    assert_eq!(fs::read(store.journal_path()).unwrap(), journal);
}

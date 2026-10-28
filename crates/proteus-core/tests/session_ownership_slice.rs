//! Conservative ownership qualification. These tests use real child processes,
//! temporary workspaces and public SessionStore APIs, not a model of locking.
//! An outer controller must impose a whole-test process-group deadline.
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use proteus_contracts::{
    domain::{SessionId, new_session_id, new_thread_id},
    model_standard::{CanonicalMessage, MessageRole},
};
use proteus_core::core::{SessionStore, delete_workspace_session};

const ENV: &str = "PROTEUS_OWNERSHIP_SLICE_HELPER";
const READY_LIMIT: Duration = Duration::from_secs(10);

struct Fixture {
    _config: tempfile::TempDir,
    _workspace: tempfile::TempDir,
    config: PathBuf,
    workspace: PathBuf,
    session: PathBuf,
    id: SessionId,
}

impl Fixture {
    async fn persisted() -> Self {
        let config = tempfile::tempdir().expect("config");
        let workspace = tempfile::tempdir().expect("workspace");
        let id = new_session_id();
        let store = SessionStore::new(config.path(), workspace.path(), id).expect("new");
        append(&store, "seed").await.expect("seed");
        let session = store.session_dir().to_path_buf();
        drop(store);
        Self {
            config: config.path().to_path_buf(),
            workspace: workspace.path().to_path_buf(),
            _config: config,
            _workspace: workspace,
            session,
            id,
        }
    }

    async fn delete(&self) -> anyhow::Result<bool> {
        delete_workspace_session(&self.config, &self.workspace, self.session.clone()).await
    }

    fn open(&self) -> SessionStore {
        SessionStore::open(self.session.clone()).expect("open existing")
    }
}

async fn append(store: &SessionStore, text: &str) -> anyhow::Result<()> {
    store
        .append_history(
            new_thread_id(),
            None,
            &[CanonicalMessage::text(MessageRole::User, text)],
        )
        .await
}

struct ChildGuard {
    child: Child,
    result: PathBuf,
}

impl ChildGuard {
    fn spawn(fixture: &Fixture, mode: &str, label: &str) -> Self {
        let result = fixture.config.join(format!("{label}.json"));
        let child = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "ownership_slice_process_helper",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(ENV, mode)
            .env("PROTEUS_OWNERSHIP_CONFIG", &fixture.config)
            .env("PROTEUS_OWNERSHIP_WORKSPACE", &fixture.workspace)
            .env("PROTEUS_OWNERSHIP_ID", fixture.id.to_string())
            .env("PROTEUS_OWNERSHIP_RESULT", &result)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ownership helper");
        Self { child, result }
    }

    fn wait_report(&mut self, must_exit: bool) -> serde_json::Value {
        let deadline = Instant::now() + READY_LIMIT;
        loop {
            let exited = self.child.try_wait().expect("poll child");
            if self.result.exists() && (!must_exit || exited.is_some()) {
                if let Some(status) = exited {
                    assert!(status.success(), "ownership helper failed: {status}");
                }
                let mut bytes = Vec::new();
                std::fs::File::open(&self.result)
                    .expect("open helper report")
                    .take(4097)
                    .read_to_end(&mut bytes)
                    .expect("read helper report");
                assert!(bytes.len() <= 4096, "helper report exceeded bound");
                return serde_json::from_slice(&bytes).expect("helper JSON");
            }
            assert!(exited.is_none(), "ownership helper exited without report");
            assert!(
                Instant::now() < deadline,
                "ownership helper readiness timed out"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        // RAII also runs on assertion panic. The controller supplies the outer
        // deadline; no helper is intentionally left alive after a test failure.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "invoked only by the bounded ownership child protocol"]
fn ownership_slice_process_helper() {
    let Some(mode) = std::env::var_os(ENV) else {
        return;
    };
    let config = PathBuf::from(std::env::var_os("PROTEUS_OWNERSHIP_CONFIG").expect("config"));
    let workspace =
        PathBuf::from(std::env::var_os("PROTEUS_OWNERSHIP_WORKSPACE").expect("workspace"));
    let id = std::env::var("PROTEUS_OWNERSHIP_ID")
        .expect("id")
        .parse()
        .expect("UUID");
    let result = PathBuf::from(std::env::var_os("PROTEUS_OWNERSHIP_RESULT").expect("result"));
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let store = SessionStore::new(&config, &workspace, id).expect("helper store");
    let attempt = runtime.block_on(append(&store, "child"));
    let report = match attempt {
        Ok(()) => serde_json::json!({"ok":true}),
        Err(error) => serde_json::json!({"ok":false,"error":format!("{error:#}")}),
    };
    let temp = result.with_extension("tmp");
    std::fs::write(&temp, serde_json::to_vec(&report).expect("report")).expect("write report");
    std::fs::rename(temp, &result).expect("publish report");
    if mode == "hold" {
        assert_eq!(report["ok"], true, "holder failed to acquire writer");
        // Finite even if the supervising process disappears unexpectedly.
        thread::sleep(Duration::from_secs(30));
    }
}

#[tokio::test]
async fn foreign_live_writer_blocks_deletion() {
    let fixture = Fixture::persisted().await;
    let mut owner = ChildGuard::spawn(&fixture, "hold", "foreign-owner");
    assert_eq!(owner.wait_report(false)["ok"], true);
    let before = std::fs::read(fixture.session.join("journal.jsonl")).expect("before");
    let result = fixture.delete().await;
    assert!(
        result.is_err(),
        "OWNERSHIP_FOREIGN_DELETE: active writer was deleted"
    );
    assert!(
        format!("{:#}", result.unwrap_err()).contains("active writer in another process"),
        "busy error must identify ownership"
    );
    assert_eq!(
        std::fs::read(fixture.session.join("journal.jsonl")).expect("after"),
        before
    );
    assert!(owner.child.try_wait().expect("owner status").is_none());
}

#[tokio::test]
async fn local_live_writer_blocks_deletion_without_deadlock() {
    let fixture = Fixture::persisted().await;
    let owner = fixture.open();
    append(&owner, "owned").await.expect("own");
    let result = tokio::time::timeout(Duration::from_secs(3), fixture.delete())
        .await
        .expect("local deletion must not wait for its own lease");
    assert!(
        result.is_err(),
        "OWNERSHIP_LOCAL_DELETE: local writer was deleted"
    );
    drop(owner);
    assert!(fixture.delete().await.expect("delete after owner drop"));
}

#[tokio::test]
async fn closed_owner_can_be_deleted() {
    let fixture = Fixture::persisted().await;
    assert!(fixture.delete().await.expect("delete closed session"));
    assert!(!fixture.session.exists());
    assert!(!fixture.delete().await.expect("second delete is absent"));
}

#[tokio::test]
async fn cold_existing_handle_cannot_recreate_deleted_session() {
    let fixture = Fixture::persisted().await;
    let cold = fixture.open();
    assert!(fixture.delete().await.expect("cold delete"));
    let result = append(&cold, "must not resurrect").await;
    assert!(
        result.is_err(),
        "OWNERSHIP_COLD_MISSING: existing handle recreated session"
    );
    assert!(
        !fixture.session.exists(),
        "existing append created a directory"
    );
}

#[tokio::test]
async fn new_for_existing_slot_cannot_recreate_deleted_session() {
    let fixture = Fixture::persisted().await;
    let cold = SessionStore::new(&fixture.config, &fixture.workspace, fixture.id)
        .expect("new on existing identity");
    assert!(fixture.delete().await.expect("cold delete"));
    let result = append(&cold, "must not resurrect").await;
    assert!(
        result.is_err(),
        "OWNERSHIP_NEW_EXISTING: existing constructor recreated session"
    );
    assert!(
        !fixture.session.exists(),
        "existing constructor created a directory"
    );
}

#[tokio::test]
async fn failed_new_materialization_consumes_creation_permission() {
    let config = tempfile::tempdir().expect("config");
    let workspace = tempfile::tempdir().expect("workspace");
    let store = SessionStore::new(config.path(), workspace.path(), new_session_id()).expect("new");
    // Deterministically fail workspace identity decoding after session mkdir.
    std::fs::remove_dir(workspace.path()).expect("remove fixture workspace");
    assert!(append(&store, "first").await.is_err());
    assert!(store.session_dir().is_dir(), "fixture must reach mkdir");
    std::fs::remove_dir(store.session_dir()).expect("remove empty failed fixture");
    std::fs::create_dir(workspace.path()).expect("restore fixture workspace");
    let result = append(&store, "retry").await;
    assert!(
        result.is_err(),
        "OWNERSHIP_NEW_CONSUMED: failed first mkdir granted recreation"
    );
    assert!(!store.session_dir().exists());
}

#[tokio::test]
async fn read_only_open_does_not_claim_writer() {
    let fixture = Fixture::persisted().await;
    let mut owner = ChildGuard::spawn(&fixture, "hold", "read-owner");
    assert_eq!(owner.wait_report(false)["ok"], true);
    let cold = fixture.open();
    assert!(!cold.load_messages().expect("read while owned").is_empty());
    assert!(append(&cold, "denied").await.is_err());
    assert!(owner.child.try_wait().expect("owner status").is_none());
}

fn replace_identity(path: &Path) -> Vec<u8> {
    let original = std::fs::read(path).expect("metadata");
    let mut value: serde_json::Value = serde_json::from_slice(&original).expect("metadata JSON");
    let id: SessionId = serde_json::from_value(value["session_id"].clone()).expect("metadata id");
    // A distinct UUID with the same 10-digit directory suffix.
    let other = SessionId::from_u128(id.as_u128().checked_add(10_000_000_000).expect("UUID room"));
    value["session_id"] = serde_json::to_value(other).expect("new id");
    std::fs::write(path, serde_json::to_vec(&value).expect("changed metadata")).expect("change id");
    original
}

#[tokio::test]
async fn rejected_cold_identity_releases_tentative_lease() {
    let fixture = Fixture::persisted().await;
    let cold = fixture.open();
    let metadata = fixture.session.join("session.json");
    let original = replace_identity(&metadata);
    assert!(append(&cold, "denied identity").await.is_err());
    std::fs::write(metadata, original).expect("restore metadata");
    let mut contender = ChildGuard::spawn(&fixture, "attempt", "identity-contender");
    assert_eq!(
        contender.wait_report(true)["ok"],
        true,
        "OWNERSHIP_TENTATIVE_IDENTITY: rejected cold handle retained lease"
    );
    // Keep the rejected handle alive through the competing process's append.
    assert!(
        !cold
            .load_messages()
            .expect("still-live cold read")
            .is_empty()
    );
}

#[tokio::test]
async fn rejected_cold_recovery_releases_tentative_lease() {
    let fixture = Fixture::persisted().await;
    let cold = fixture.open();
    let journal = fixture.session.join("journal.jsonl");
    let original = std::fs::read(&journal).expect("journal");
    std::fs::write(&journal, b"invalid-json\n").expect("corrupt fixture");
    assert!(append(&cold, "denied recovery").await.is_err());
    std::fs::write(&journal, original).expect("restore fixture");
    let mut contender = ChildGuard::spawn(&fixture, "attempt", "recovery-contender");
    assert_eq!(
        contender.wait_report(true)["ok"],
        true,
        "OWNERSHIP_TENTATIVE_RECOVERY: rejected cold recovery retained lease"
    );
    assert!(
        !cold
            .load_messages()
            .expect("still-live cold read")
            .is_empty()
    );
}

#[tokio::test]
async fn rejected_existing_owner_preserves_legitimate_lease() {
    let fixture = Fixture::persisted().await;
    let owner = fixture.open();
    append(&owner, "legitimate owner").await.expect("acquire");
    let metadata = fixture.session.join("session.json");
    let original = replace_identity(&metadata);
    assert!(append(&owner, "denied identity").await.is_err());
    std::fs::write(metadata, original).expect("restore metadata");
    let mut contender = ChildGuard::spawn(&fixture, "attempt", "retained-contender");
    let report = contender.wait_report(true);
    assert_eq!(
        report["ok"], false,
        "OWNERSHIP_RETAINED: valid owner lost lease"
    );
    assert!(
        report["error"]
            .as_str()
            .expect("error")
            .contains("active writer in another process")
    );
    append(&owner, "owner still works")
        .await
        .expect("retained writer");
}

#[test]
fn constructing_new_session_remains_lazy() {
    let config = tempfile::tempdir().expect("config");
    let workspace = tempfile::tempdir().expect("workspace");
    let store = SessionStore::new(config.path(), workspace.path(), new_session_id()).expect("new");
    assert!(!store.session_dir().exists());
    assert!(!config.path().join("sessions").exists());
}

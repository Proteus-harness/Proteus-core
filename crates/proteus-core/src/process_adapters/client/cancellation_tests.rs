use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use proteus_module_protocol::{ProcessExportBinding, v3::NoAsyncHostRequests};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::timeout,
};

use super::{ProcessExportClient, ProcessInvocationError, ProcessInvocationFailure};
use crate::{
    contracts::{PROCESS_CONTEXT_BUILD_METHOD, PROCESS_CONTEXT_CONTRACT_VERSION},
    process_adapters::{ProcessComponentConfig, ProcessComponentLauncher},
};

fn client(workspace: &Path, env: Value) -> Arc<ProcessExportClient> {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/admission_cancellation.py");
    let config: ProcessComponentConfig = serde_json::from_value(json!({
        "command": "python3",
        "args": [fixture],
        "env": env,
        "handshake_timeout_ms": 3_000,
        "exports": {"context": {"cancel-probe": {"timeout_ms": 3_000}}}
    }))
    .unwrap();
    let binding = ProcessExportBinding::new(
        "context",
        "cancel-probe",
        PROCESS_CONTEXT_CONTRACT_VERSION,
        json!({}),
    )
    .unwrap();
    let launcher = ProcessComponentLauncher::new("cancel-probe", config, vec![binding]).unwrap();
    Arc::new(
        ProcessExportClient::connect(
            "context",
            PROCESS_CONTEXT_CONTRACT_VERSION,
            launcher.export("context", "cancel-probe").unwrap(),
            workspace,
            3_000,
        )
        .unwrap(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_during_restart_admission_returns_before_handshake_and_never_dispatches() {
    let workspace = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let client = client(
        workspace.path(),
        json!({"PROTEUS_TEST_RESTART_HANDSHAKE_PORT": listener.local_addr().unwrap().port().to_string()}),
    );
    client.reset();
    let started = workspace.path().join("abandoned-started");
    let cancellation = Arc::new(AtomicBool::new(false));
    let mut invocation = {
        let client = Arc::clone(&client);
        let cancellation = Arc::clone(&cancellation);
        let started = started.clone();
        tokio::spawn(async move {
            client
                .invoke_with_dispatcher_and_cancel_check::<_, Value, _>(
                    PROCESS_CONTEXT_BUILD_METHOD,
                    &json!({"started_path": started}),
                    Arc::new(NoAsyncHostRequests),
                    || cancellation.load(Ordering::SeqCst),
                )
                .await
        })
    };
    let (mut gate, _) = timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("replacement worker must reach handshake")
        .unwrap();
    let mut ready = [0; 6];
    timeout(Duration::from_secs(1), gate.read_exact(&mut ready))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&ready, b"ready\n");
    cancellation.store(true, Ordering::SeqCst);
    let canceled = timeout(Duration::from_millis(500), &mut invocation).await;
    // Always release the worker, including the pre-fix timeout failure path.
    gate.write_all(b"1").await.unwrap();
    let error = match canceled {
        Ok(result) => result.unwrap().expect_err("canceled admission must fail"),
        Err(_) => {
            invocation.abort();
            panic!("cancellation remained blocked by the replacement handshake");
        }
    };
    assert!(format!("{error:#}").contains("was canceled"));

    let survivor: Value = client
        .invoke(PROCESS_CONTEXT_BUILD_METHOD, &json!({"value": "survived"}))
        .await
        .unwrap();
    assert_eq!(survivor["value"], "survived");
    assert!(!started.exists(), "abandoned request reached the worker");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_after_admission_preserves_typed_terminal_and_component() {
    let workspace = tempfile::tempdir().unwrap();
    let client = client(workspace.path(), json!({}));
    let before: Value = client
        .invoke(PROCESS_CONTEXT_BUILD_METHOD, &json!({"value": "before"}))
        .await
        .unwrap();
    let started = workspace.path().join("active-started");
    let cancellation = Arc::new(AtomicBool::new(false));
    let invocation = {
        let client = Arc::clone(&client);
        let cancellation = Arc::clone(&cancellation);
        let started = started.clone();
        tokio::spawn(async move {
            client
                .invoke_with_dispatcher_and_cancel_check::<_, Value, _>(
                    PROCESS_CONTEXT_BUILD_METHOD,
                    &json!({"op": "wait_cancel", "started_path": started}),
                    Arc::new(NoAsyncHostRequests),
                    || cancellation.load(Ordering::SeqCst),
                )
                .await
        })
    };
    timeout(Duration::from_secs(1), async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("worker must start before cancellation");
    cancellation.store(true, Ordering::SeqCst);
    let error = timeout(Duration::from_secs(1), invocation)
        .await
        .unwrap()
        .unwrap()
        .expect_err("cooperative cancellation");
    assert_eq!(
        error
            .downcast_ref::<ProcessInvocationError>()
            .unwrap()
            .failure(),
        &ProcessInvocationFailure::Canceled
    );
    let after: Value = client
        .invoke(PROCESS_CONTEXT_BUILD_METHOD, &json!({"value": "after"}))
        .await
        .unwrap();
    assert_eq!(
        after["pid"], before["pid"],
        "cooperative cancel reset component"
    );
    assert_eq!(after["value"], "after");
}

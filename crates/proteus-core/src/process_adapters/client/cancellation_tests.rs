use std::{path::Path, sync::Arc, time::Duration};

use proteus_module_protocol::{ProcessExportBinding, v3::NoAsyncHostRequests};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::timeout,
};

use super::{ProcessExportClient, ProcessInvocationError, ProcessInvocationFailure};
use crate::{
    contracts::{
        CancellationToken, PROCESS_CONTEXT_BUILD_METHOD, PROCESS_CONTEXT_CONTRACT_VERSION,
    },
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
    restart_admission_cancellation(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_cancellation_during_restart_admission_never_dispatches() {
    restart_admission_cancellation(true).await;
}

async fn restart_admission_cancellation(event_driven: bool) {
    let workspace = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let client = client(
        workspace.path(),
        json!({"PROTEUS_TEST_RESTART_HANDSHAKE_PORT": listener.local_addr().unwrap().port().to_string()}),
    );
    client.reset();
    let started = workspace.path().join("abandoned-started");
    let cancellation = CancellationToken::new();
    let mut invocation = {
        let client = Arc::clone(&client);
        let cancellation = cancellation.clone();
        let started = started.clone();
        tokio::spawn(async move {
            invoke_cancellable(
                &client,
                &json!({"started_path": started}),
                &cancellation,
                event_driven,
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
    cancellation.cancel();
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
    active_cancellation(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_cancellation_after_admission_preserves_typed_terminal_and_component() {
    active_cancellation(true).await;
}

async fn active_cancellation(event_driven: bool) {
    let workspace = tempfile::tempdir().unwrap();
    let client = client(workspace.path(), json!({}));
    let before: Value = client
        .invoke(PROCESS_CONTEXT_BUILD_METHOD, &json!({"value": "before"}))
        .await
        .unwrap();
    let started = workspace.path().join("active-started");
    let cancellation = CancellationToken::new();
    let invocation = {
        let client = Arc::clone(&client);
        let cancellation = cancellation.clone();
        let started = started.clone();
        tokio::spawn(async move {
            invoke_cancellable(
                &client,
                &json!({"op": "wait_cancel", "started_path": started}),
                &cancellation,
                event_driven,
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
    cancellation.cancel();
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

async fn invoke_cancellable(
    client: &ProcessExportClient,
    request: &Value,
    cancellation: &CancellationToken,
    event_driven: bool,
) -> anyhow::Result<Value> {
    if event_driven {
        client
            .invoke_with_dispatcher_and_cancellation(
                PROCESS_CONTEXT_BUILD_METHOD,
                request,
                Arc::new(NoAsyncHostRequests),
                cancellation,
            )
            .await
    } else {
        client
            .invoke_with_dispatcher_and_cancel_check(
                PROCESS_CONTEXT_BUILD_METHOD,
                request,
                Arc::new(NoAsyncHostRequests),
                || cancellation.is_cancelled(),
            )
            .await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn already_canceled_token_does_not_serialize_or_dispatch() {
    struct MustNotSerialize;
    impl serde::Serialize for MustNotSerialize {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            panic!("pre-canceled invocation must not encode params");
        }
    }

    let workspace = tempfile::tempdir().unwrap();
    let client = client(workspace.path(), json!({}));
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let result: anyhow::Result<Value> = client
        .invoke_with_dispatcher_and_cancellation(
            PROCESS_CONTEXT_BUILD_METHOD,
            &MustNotSerialize,
            Arc::new(NoAsyncHostRequests),
            &cancellation,
        )
        .await;
    let error = result.expect_err("pre-canceled invocation");
    assert!(format!("{error:#}").contains("was canceled"));
}

#[test]
fn token_wait_needs_no_timer_and_observes_cancel_between_phases() {
    use super::cancellation::CancellationSource;
    use std::task::Poll;

    // A token wait must work with no time driver. A periodic cancellation
    // timer here would panic rather than silently adding wakeups again.
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(async {
            let cancellation = CancellationToken::new();
            let mut source = CancellationSource::<fn() -> bool>::Token(&cancellation);
            source.begin_wait();
            {
                let pending = source.observe();
                tokio::pin!(pending);
                assert!(matches!(futures_util::poll!(&mut pending), Poll::Pending));
                // Admission completed; its cancellation waiter is dropped.
            }
            cancellation.cancel();
            // The terminal phase must observe cancellation even though no
            // waiter existed at the moment it happened.
            source.begin_wait();
            assert!(source.observe().await);
        });
}

#[tokio::test]
async fn bool_wait_returns_each_false_observation_to_the_outer_select() {
    use super::cancellation::CancellationSource;
    use std::cell::Cell;

    // Cell also retains the old Fn-only API: a local caller need not provide
    // a Sync predicate merely because token callers use async_trait adapters.
    let observations = Cell::new(0);
    let mut source = CancellationSource::Check {
        check: || {
            observations.set(observations.get() + 1);
            false
        },
        poll: None,
    };
    source.begin_wait();
    assert!(
        !timeout(Duration::from_millis(500), source.observe())
            .await
            .expect("a false observation must not wait for cancellation")
    );
    assert_eq!(observations.get(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_cancel_grace_preserves_uncanceled_sibling_component_loss() {
    use proteus_module_protocol::v3::ComponentFailure;

    let workspace = tempfile::tempdir().unwrap();
    let client = client(workspace.path(), json!({}));
    let cancellation = CancellationToken::new();
    let started = workspace.path().join("uncooperative-started");
    let sibling_started = workspace.path().join("sibling-started");
    let target = {
        let client = client.clone();
        let cancellation = cancellation.clone();
        let started = started.clone();
        tokio::spawn(async move {
            invoke_cancellable(
                &client,
                &json!({"op": "ignore_cancel", "started_path": started}),
                &cancellation,
                true,
            )
            .await
        })
    };
    let sibling = {
        let client = client.clone();
        let started = sibling_started.clone();
        tokio::spawn(async move {
            invoke_cancellable(
                &client,
                &json!({"op": "wait_cancel", "started_path": started}),
                &CancellationToken::new(),
                true,
            )
            .await
        })
    };
    timeout(Duration::from_secs(1), async {
        while !started.exists() || !sibling_started.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("both invocations must be dispatched before cancellation");
    cancellation.cancel();
    let target_error = timeout(Duration::from_secs(2), target)
        .await
        .unwrap()
        .unwrap()
        .expect_err("uncooperative target");
    assert_eq!(
        target_error
            .downcast_ref::<ProcessInvocationError>()
            .unwrap()
            .failure(),
        &ProcessInvocationFailure::Canceled
    );
    let sibling_error = timeout(Duration::from_secs(2), sibling)
        .await
        .unwrap()
        .unwrap()
        .expect_err("sibling shares the lost generation");
    assert_eq!(
        sibling_error
            .downcast_ref::<ProcessInvocationError>()
            .unwrap()
            .failure(),
        &ProcessInvocationFailure::ComponentLost(ComponentFailure::CancelGrace)
    );
}

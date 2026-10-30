use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use proteus_contracts::contracts::{
    MODEL_CATALOG_FEATURE, MODEL_QUOTA_FEATURE, PROCESS_MODEL_CATALOG_METHOD,
    PROCESS_MODEL_CONTRACT_VERSION, PROCESS_MODEL_DESCRIBE_METHOD, PROCESS_MODEL_QUOTA_METHOD,
    PROCESS_WORKFLOW_CONTRACT_VERSION, PROCESS_WORKFLOW_METHOD,
};
use proteus_process_host::ProcessSpec;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::v3::{
    broker::{StartAck, StartRequest},
    config::ComponentBrokerOptions,
    invocation::{
        CancelCause, ComponentBrokerError, ComponentBrokerErrorKind, ComponentFailure,
        InvocationRef, InvocationTerminal, NoAsyncHostRequests, StartMeta,
    },
    notification::NotificationSink,
    pending::{LoopState, TerminalSender},
};
use crate::{ProcessComponentBinding, ProcessExportBinding};

const WAIT: Duration = Duration::from_secs(3);

fn state(spec: ProcessSpec) -> Result<LoopState> {
    let binding = ProcessComponentBinding::new(
        "admission",
        [ProcessExportBinding::new(
            "workflow",
            "fixture.workflow",
            PROCESS_WORKFLOW_CONTRACT_VERSION,
            json!({}),
        )?],
    )?;
    let options = ComponentBrokerOptions {
        handshake_timeout: Duration::from_secs(5),
        max_active_roots: 1,
        ..ComponentBrokerOptions::default()
    };
    options.validate()?;
    let wake = proteus_process_host::ProcessWake::default();
    let (control_tx, _) = crate::v3::signal::channel(options.control_command_capacity, &wake);
    Ok(LoopState::new(spec, binding, options, control_tx, wake))
}

fn fixture() -> ProcessSpec {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multiplex_worker.py");
    ProcessSpec::new("python3")
        .arg("-B")
        .arg(path.to_string_lossy().into_owned())
}

fn request(
    state: &LoopState,
    params: Value,
    parent: Option<InvocationRef>,
) -> (
    StartRequest,
    mpsc::Receiver<Result<StartMeta, ComponentBrokerError>>,
    mpsc::Receiver<InvocationTerminal>,
) {
    let (terminal_tx, terminal_rx) = mpsc::channel();
    let (ack_tx, ack_rx) = mpsc::channel();
    let (notifications, _) = NotificationSink::channel(state.options.notification_limits);
    (
        StartRequest {
            target: state.binding.exports[0].export_ref(),
            method: PROCESS_WORKFLOW_METHOD.to_owned(),
            params,
            deadline: Instant::now() + Duration::from_secs(30),
            parent,
            dispatcher: Arc::new(NoAsyncHostRequests),
            executor: None,
            terminal: TerminalSender::Blocking(terminal_tx),
            notifications,
            ack: StartAck::Blocking(ack_tx),
            bootstrap: false,
        },
        ack_rx,
        terminal_rx,
    )
}

fn admit(
    state: &mut LoopState,
    params: Value,
    parent: Option<InvocationRef>,
) -> Result<(StartMeta, mpsc::Receiver<InvocationTerminal>)> {
    let (request, ack, terminal) = request(state, params, parent);
    state.accept_start(request);
    Ok((ack.recv_timeout(WAIT)??, terminal))
}

fn receive(state: &mut LoopState) -> Result<Value> {
    Ok(state
        .worker
        .as_mut()
        .context("worker disappeared")?
        .transport
        .recv_frame(WAIT)?)
}

fn assert_clean(state: &LoopState) -> Result<()> {
    ensure!(state.pending.is_empty() && state.queued_roots.is_empty());
    ensure!(state.active_roots == 0 && state.active_nested == 0);
    ensure!(state.callbacks.is_empty() && state.pending_callback_counts.is_empty());
    Ok(())
}

fn terminal(
    receiver: &mpsc::Receiver<InvocationTerminal>,
    expected: InvocationTerminal,
) -> Result<()> {
    ensure!(receiver.recv_timeout(WAIT)? == expected);
    ensure!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    Ok(())
}

#[test]
fn abandoned_start_during_restart_is_not_dispatched() -> Result<()> {
    check_abandoned_restart(false)
}

#[test]
fn abandoned_optional_start_during_restart_is_not_dispatched() -> Result<()> {
    check_abandoned_restart(true)
}

fn check_abandoned_restart(optional_method: bool) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    let spec = fixture().env(
        "PROTEUS_TEST_RESTART_HANDSHAKE_PORT",
        listener.local_addr()?.port().to_string(),
    );
    let mut state = if optional_method {
        feature_state(spec)?
    } else {
        state(spec)?
    };
    state.ensure_worker()?;
    state.reset_generation(ComponentFailure::Shutdown);
    ensure!(state.generation == 2);
    let (mut request, _, _) = request(&state, json!({"op": "wait_cancel"}), None);
    if optional_method {
        request.method = PROCESS_MODEL_QUOTA_METHOD.to_owned();
    }
    let (terminal_tx, terminal_rx) = oneshot::channel();
    let (ack_tx, ack_rx) = oneshot::channel();
    request.terminal = TerminalSender::Async(terminal_tx);
    request.ack = StartAck::Async(ack_tx);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let admission = thread::spawn(move || {
        state.accept_start(request);
        let _ = done_tx.send(state);
    });
    let gate_result = (|| -> Result<()> {
        let mut gate = accept_barrier(&listener)?;
        gate.set_read_timeout(Some(WAIT))?;
        gate.set_write_timeout(Some(WAIT))?;
        let mut ready = [0_u8; 6];
        gate.read_exact(&mut ready)?;
        ensure!(&ready == b"ready\n");
        drop(terminal_rx);
        drop(ack_rx);
        gate.write_all(b"1")?;
        Ok(())
    })();
    // This watchdog bounds the test's wait, not reclamation of a wedged
    // admission thread. Handshake/socket deadlines and transport Drop cover
    // ordinary exits; run this target under an outer process watchdog.
    let mut state = done_rx
        .recv_timeout(Duration::from_secs(8))
        .context("restart admission did not finish within watchdog")?;
    admission.join().expect("admission thread panicked");
    let dispatched = state
        .pending
        .values()
        .any(|pending| pending.dispatch.is_some());
    let clean = assert_clean(&state);
    let generation = state.generation;
    state.reset_generation(ComponentFailure::Shutdown);
    gate_result?;
    ensure!(
        generation == 2,
        "handshake failed instead of exercising admission"
    );
    ensure!(
        !dispatched,
        "abandoned restart admission reached the frame writer"
    );
    clean?;
    Ok(())
}

#[test]
fn undeliverable_blocking_start_ack_never_dispatches() -> Result<()> {
    check_undeliverable_start_ack(false)
}

#[test]
fn undeliverable_async_start_ack_never_dispatches() -> Result<()> {
    check_undeliverable_start_ack(true)
}

fn check_undeliverable_start_ack(asynchronous_ack: bool) -> Result<()> {
    let mut state = state(fixture())?;
    let (mut request, ack, result) = request(&state, json!({"op": "wait_cancel"}), None);
    drop(ack);
    if asynchronous_ack {
        let (sender, receiver) = oneshot::channel();
        request.ack = StartAck::Async(sender);
        drop(receiver);
    }
    state.accept_start(request);
    ensure!(state.generation == 1 && state.worker.is_some());
    // Preserve the existing bootstrap-closing point after validation.
    ensure!(state.runtime_started);
    ensure!(
        !state
            .pending
            .values()
            .any(|pending| pending.dispatch.is_some()),
        "undeliverable start ACK reached the frame writer (async={asynchronous_ack})"
    );
    assert_clean(&state).context("undeliverable start ACK retained invocation state")?;
    terminal(&result, InvocationTerminal::Canceled)?;

    let (_, result) = admit(&mut state, json!({"op": "echo", "value": "live"}), None)?;
    let response = receive(&mut state)?;
    state.handle_frame(response);
    ensure!(matches!(
        result.recv_timeout(WAIT)?,
        InvocationTerminal::Success(value) if value["value"] == "live"
    ));
    assert_clean(&state)?;
    state.reset_generation(ComponentFailure::Shutdown);
    Ok(())
}

#[test]
fn queued_abandonment_cancels_nested_work_and_promotes_live_follower() -> Result<()> {
    check_queued_rejection(true)
}

#[test]
fn queued_expiry_cancels_nested_work_and_promotes_live_follower() -> Result<()> {
    check_queued_rejection(false)
}

fn check_queued_rejection(abandoned: bool) -> Result<()> {
    let mut state = state(fixture())?;
    let (incumbent, incumbent_result) =
        admit(&mut state, json!({"op": "echo", "value": "first"}), None)?;
    let (mut queued, ack, _) = request(&state, json!({"op": "wait_cancel"}), None);
    let (sender, receiver) = oneshot::channel();
    queued.terminal = TerminalSender::Async(sender);
    state.accept_start(queued);
    let queued = ack.recv_timeout(WAIT)??;
    ensure!(!state.pending[queued.invocation.id()].active);
    let (child, child_result) = admit(
        &mut state,
        json!({"op": "wait_cancel", "notify_started": true}),
        Some(queued.invocation.clone()),
    )?;
    let (follower, follower_result) =
        admit(&mut state, json!({"op": "echo", "value": "last"}), None)?;
    ensure!(!state.pending[follower.invocation.id()].active);

    // Hold the incumbent response so queue promotion cannot run yet.
    // The child progress frame proves that its writer claim already happened.
    let mut incumbent_response = None;
    let mut child_started = false;
    for _ in 0..2 {
        let frame = receive(&mut state)?;
        if frame["id"].as_str() == Some(incumbent.invocation.id()) {
            incumbent_response = Some(frame);
        } else {
            ensure!(frame["method"] == "module.progress");
            ensure!(frame["params"]["invocation_id"].as_str() == Some(child.invocation.id()));
            state.handle_frame(frame);
            child_started = true;
        }
    }
    ensure!(child_started && incumbent_response.is_some());
    ensure!(state.pending[child.invocation.id()].is_visible_to_worker());

    let mut receiver = Some(receiver);
    let cause = if abandoned {
        drop(receiver.take());
        CancelCause::User
    } else {
        state
            .pending
            .get_mut(queued.invocation.id())
            .unwrap()
            .invocation
            .deadline = Instant::now();
        CancelCause::Timeout
    };
    // Exercise the actual finish -> queued promotion path, without a tick
    // of enforce_deadlines that could hide the activation bug.
    state.handle_frame(incumbent_response.unwrap());
    ensure!(
        !state.pending.contains_key(queued.invocation.id()),
        "rejected queued parent reached activation (abandoned={abandoned})"
    );
    ensure!(
        state.pending[child.invocation.id()].cancel == Some(cause),
        "rejected queued parent orphaned its active child (abandoned={abandoned})"
    );
    ensure!(
        state.pending[follower.invocation.id()].active,
        "rejected queued parent blocked its live follower"
    );
    ensure!(state.pending[follower.invocation.id()].dispatch.is_some());
    ensure!(state.active_roots == 1 && state.active_nested == 1);
    if let Some(mut receiver) = receiver {
        ensure!(receiver.try_recv()? == InvocationTerminal::TimedOut);
        ensure!(matches!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
    }
    // Child cancellation and the follower each produce exactly one terminal.
    for _ in 0..2 {
        let frame = receive(&mut state)?;
        state.handle_frame(frame);
    }
    terminal(&child_result, InvocationTerminal::canceled(cause))?;
    ensure!(matches!(
        incumbent_result.recv_timeout(WAIT)?,
        InvocationTerminal::Success(value) if value["value"] == "first"
    ));
    ensure!(matches!(
        follower_result.recv_timeout(WAIT)?,
        InvocationTerminal::Success(value) if value["value"] == "last"
    ));
    assert_clean(&state).context("queued rejection leaked invocation or callback budget")?;
    ensure!(state.generation == 1);
    state.reset_generation(ComponentFailure::Shutdown);
    Ok(())
}

fn accept_barrier(listener: &TcpListener) -> Result<TcpStream> {
    let deadline = Instant::now() + WAIT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                ensure!(
                    Instant::now() < deadline,
                    "restart handshake did not reach barrier"
                );
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return Err(error).context("accept restart handshake barrier"),
        }
    }
}

fn feature_state(spec: ProcessSpec) -> Result<LoopState> {
    let mut state = state(spec)?;
    state.binding = ProcessComponentBinding::new(
        "feature-admission",
        [
            ProcessExportBinding::new(
                "model",
                "changing",
                PROCESS_MODEL_CONTRACT_VERSION,
                json!({"features_by_generation": {
                    "1": [MODEL_QUOTA_FEATURE], "2": [], "3": [MODEL_CATALOG_FEATURE]
                }}),
            )?,
            ProcessExportBinding::new(
                "model",
                "sibling",
                PROCESS_MODEL_CONTRACT_VERSION,
                json!({"features": [MODEL_CATALOG_FEATURE]}),
            )?,
            ProcessExportBinding::new(
                "workflow",
                "parent",
                PROCESS_WORKFLOW_CONTRACT_VERSION,
                json!({}),
            )?,
        ],
    )?;
    Ok(state)
}

fn feature_request(
    state: &LoopState,
    export: usize,
    method: &str,
    parent: Option<InvocationRef>,
) -> (
    StartRequest,
    mpsc::Receiver<Result<StartMeta, ComponentBrokerError>>,
    mpsc::Receiver<InvocationTerminal>,
) {
    let (mut request, ack, terminal) =
        request(state, json!({"op": "echo", "value": method}), parent);
    request.target = state.binding.exports[export].export_ref();
    request.method = method.to_owned();
    (request, ack, terminal)
}

fn finish_feature_call(
    state: &mut LoopState,
    export: usize,
    method: &str,
    parent: Option<InvocationRef>,
) -> Result<StartMeta> {
    let (request, ack, terminal) = feature_request(state, export, method, parent);
    state.accept_start(request);
    let meta = ack.recv_timeout(WAIT)??;
    let frame = receive(state)?;
    state.handle_frame(frame);
    ensure!(matches!(
        terminal.recv_timeout(WAIT)?,
        InvocationTerminal::Success(value) if value["value"] == method
    ));
    Ok(meta)
}

#[test]
fn optional_admission_uses_current_generation_and_exact_export_not_a_previous_manifest()
-> Result<()> {
    let mut state = feature_state(fixture())?;
    state.ensure_worker()?;
    let old_manifest = state.worker.as_ref().unwrap().manifest.clone();
    ensure!(old_manifest.exports[0].module_features == vec![MODEL_QUOTA_FEATURE.to_owned()]);
    state.reset_generation(ComponentFailure::Shutdown);

    // Mirrors a stale successful feature probe followed by lazy restart. G2
    // acknowledges no optional method for this export, but its sibling does.
    for method in [PROCESS_MODEL_QUOTA_METHOD, PROCESS_MODEL_CATALOG_METHOD] {
        let next_id = state.next_host_sequence;
        let (request, ack, terminal) = feature_request(&state, 0, method, None);
        state.accept_start(request);
        let error = ack
            .recv_timeout(WAIT)?
            .err()
            .context("expected rejection")?;
        ensure!(error.kind == ComponentBrokerErrorKind::UnsupportedFeature);
        ensure!(matches!(
            terminal.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        ensure!(state.generation == 2 && state.next_host_sequence == next_id);
        ensure!(!state.runtime_started);
        assert_clean(&state)?;
    }
    let sibling = finish_feature_call(&mut state, 1, PROCESS_MODEL_CATALOG_METHOD, None)?;
    let mandatory = finish_feature_call(&mut state, 0, PROCESS_MODEL_DESCRIBE_METHOD, None)?;
    ensure!(sibling.pid == mandatory.pid && state.generation == 2);
    assert_clean(&state)?;

    // G3 adds catalog and removes quota; neither positive nor negative
    // knowledge from an earlier generation may determine admission.
    state.reset_generation(ComponentFailure::Shutdown);
    let added = finish_feature_call(&mut state, 0, PROCESS_MODEL_CATALOG_METHOD, None)?;
    ensure!(added.invocation.generation == 3);
    let (request, ack, _) = feature_request(&state, 0, PROCESS_MODEL_QUOTA_METHOD, None);
    state.accept_start(request);
    let error = ack
        .recv_timeout(WAIT)?
        .err()
        .context("expected rejection")?;
    ensure!(error.kind == ComponentBrokerErrorKind::UnsupportedFeature);
    ensure!(state.worker.as_ref().unwrap().pid == added.pid && state.generation == 3);
    assert_clean(&state)?;
    state.reset_generation(ComponentFailure::Shutdown);
    Ok(())
}

#[test]
fn missing_nested_feature_preserves_parent_and_cancel_then_rejects_stale_parent() -> Result<()> {
    let mut state = feature_state(fixture())?;
    let (mut request, ack, parent_terminal) =
        feature_request(&state, 2, PROCESS_WORKFLOW_METHOD, None);
    request.params = json!({"op": "wait_cancel", "notify_started": true});
    state.accept_start(request);
    let parent = ack.recv_timeout(WAIT)??;
    let started = receive(&mut state)?;
    ensure!(started["method"] == "module.progress");
    state.handle_frame(started);
    ensure!(state.pending[parent.invocation.id()].is_visible_to_worker());

    let (request, ack, _) = feature_request(
        &state,
        0,
        PROCESS_MODEL_CATALOG_METHOD,
        Some(parent.invocation.clone()),
    );
    state.accept_start(request);
    let error = ack
        .recv_timeout(WAIT)?
        .err()
        .context("expected rejection")?;
    ensure!(error.kind == ComponentBrokerErrorKind::UnsupportedFeature);
    ensure!(state.pending.len() == 1 && state.active_roots == 1 && state.active_nested == 0);
    ensure!(state.pending[parent.invocation.id()].cancel.is_none());
    let child = finish_feature_call(
        &mut state,
        0,
        PROCESS_MODEL_QUOTA_METHOD,
        Some(parent.invocation.clone()),
    )?;
    ensure!(child.pid == parent.pid);
    ensure!(child.invocation.parent_id.as_deref() == Some(parent.invocation.id()));

    state.cancel(
        parent.invocation.id(),
        parent.invocation.generation,
        CancelCause::User,
    )?;
    let (request, ack, _) = feature_request(
        &state,
        0,
        PROCESS_MODEL_CATALOG_METHOD,
        Some(parent.invocation.clone()),
    );
    state.accept_start(request);
    let error = ack
        .recv_timeout(WAIT)?
        .err()
        .context("expected rejection")?;
    ensure!(error.kind == ComponentBrokerErrorKind::ParentInactive);
    let response = receive(&mut state)?;
    state.handle_frame(response);
    terminal(&parent_terminal, InvocationTerminal::Canceled)?;
    assert_clean(&state)?;
    ensure!(state.generation == 1 && state.worker.as_ref().unwrap().pid == parent.pid);
    let (request, ack, _) = feature_request(
        &state,
        0,
        PROCESS_MODEL_CATALOG_METHOD,
        Some(parent.invocation),
    );
    state.accept_start(request);
    let error = ack
        .recv_timeout(WAIT)?
        .err()
        .context("expected rejection")?;
    ensure!(error.kind == ComponentBrokerErrorKind::ParentInactive);
    assert_clean(&state)?;
    state.reset_generation(ComponentFailure::Shutdown);
    Ok(())
}

#[test]
fn invalid_restart_handshake_is_not_optional_feature_absence() -> Result<()> {
    let mut state = feature_state(fixture())?;
    state.binding.exports[0].module_config["features_by_generation"]["2"] =
        json!(["unoffered.feature"]);
    state.ensure_worker()?;
    state.reset_generation(ComponentFailure::Shutdown);
    let (request, ack, _) = feature_request(&state, 0, PROCESS_MODEL_QUOTA_METHOD, None);
    state.accept_start(request);
    let error = ack
        .recv_timeout(WAIT)?
        .err()
        .context("expected rejection")?;
    ensure!(error.kind == ComponentBrokerErrorKind::RuntimeUnavailable);
    ensure!(state.worker.is_none());
    ensure!(state.last_failure == Some(ComponentFailure::Protocol));
    assert_clean(&state)?;
    Ok(())
}

#[test]
fn expired_acknowledged_optional_start_settles_without_dispatch() -> Result<()> {
    let mut state = feature_state(fixture())?;
    let (mut request, ack, result) = feature_request(&state, 0, PROCESS_MODEL_QUOTA_METHOD, None);
    request.deadline = Instant::now();
    state.accept_start(request);
    let admitted = ack.recv_timeout(WAIT)??;
    ensure!(admitted.invocation.generation == 1);
    terminal(&result, InvocationTerminal::TimedOut)?;
    assert_clean(&state)?;
    ensure!(state.worker.is_some() && state.runtime_started);
    state.reset_generation(ComponentFailure::Shutdown);
    Ok(())
}

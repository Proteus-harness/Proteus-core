//! Ownership-only regressions: no child process or Tokio runtime is started.
use super::*;
use crate::{ProcessExportBinding, v3::pending::PendingInvocation};
use proteus_contracts::contracts::PROCESS_WORKFLOW_CONTRACT_VERSION;
use serde_json::json;

struct RetainsBroker {
    _broker: ComponentBroker,
}

impl AsyncHostRequestDispatcher for RetainsBroker {
    fn dispatch(&self, _: super::super::ComponentHostRequest) -> super::super::HostRequestFuture {
        panic!("ownership regression must not dispatch a callback")
    }
}

fn binding() -> ProcessComponentBinding {
    ProcessComponentBinding::new(
        "drop-owner",
        [ProcessExportBinding::new(
            "workflow",
            "fixture",
            PROCESS_WORKFLOW_CONTRACT_VERSION,
            json!({}),
        )
        .unwrap()],
    )
    .unwrap()
}

#[test]
fn pending_dispatcher_can_release_final_broker_owner_on_broker_thread() {
    for full in [false, true] {
        for reset in [false, true] {
            final_owner_on_broker_thread(full, reset);
        }
    }
}

fn final_owner_on_broker_thread(full: bool, reset: bool) {
    let wake = ProcessWake::default();
    let (root_tx, root_rx) = signal::channel(1, &wake);
    let (control_tx, control_rx) = signal::channel(1, &wake);
    let stopped = Arc::new(AtomicBool::new(false));
    let thread_stopped = Arc::clone(&stopped);
    let options = ComponentBrokerOptions::default();
    let binding = binding();
    let mut state = LoopState::new(
        ProcessSpec::new("must-not-start-a-process"),
        binding.clone(),
        options,
        control_tx.clone(),
        wake,
    );
    let (owner_tx, owner_rx) = mpsc::channel::<ComponentBroker>();
    let (done_tx, done_rx) = mpsc::channel();
    let thread = thread::spawn(move || {
        let broker = owner_rx.recv().unwrap();
        let (notifications, _receiver) = NotificationSink::channel(options.notification_limits);
        let id = "h:1:1".to_owned();
        state.pending.insert(
            id.clone(),
            PendingInvocation {
                invocation: InvocationRef {
                    id: id.clone(),
                    generation: 1,
                    target: state.binding.exports[0].export_ref(),
                    root_id: id.clone(),
                    parent_id: None,
                    depth: 0,
                    deadline: Instant::now() + Duration::from_secs(60),
                },
                method: "workflow.run".to_owned(),
                params: None,
                authority: *state.binding.exports[0].authority().unwrap(),
                dispatcher: Arc::new(RetainsBroker { _broker: broker }),
                executor: None,
                terminal: None,
                notifications,
                dispatch: None,
                active: false,
                cancel: None,
                cancel_deadline: None,
                outstanding_callbacks: Default::default(),
            },
        );
        if reset {
            state.reset_generation(ComponentFailure::Protocol);
        } else {
            state.finish_without_admission(&id, InvocationTerminal::Canceled);
        }
        assert!(thread_stopped.load(Ordering::Acquire));
        // The same real loop must exit without attempting another initialization.
        state.run(root_rx, control_rx, &thread_stopped);
        assert!(state.pending.is_empty());
        assert_eq!(state.last_failure, Some(ComponentFailure::Shutdown));
        done_tx.send(()).unwrap();
    });
    let broker = ComponentBroker {
        inner: Arc::new(BrokerInner {
            root_tx,
            control_tx,
            binding,
            options,
            stopped,
            thread: Mutex::new(Some(thread)),
        }),
    };
    let weak = broker.downgrade();
    if full {
        let (ack, _receiver) = mpsc::channel();
        assert!(
            broker
                .inner
                .control_tx
                .try_send(ControlCommand::Inspect { ack })
                .is_ok()
        );
    }
    owner_tx.send(broker).unwrap();
    // A failed old implementation may leave its thread blocked on a full queue;
    // this watchdog bounds the test, not reclamation of that failed thread.
    done_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("broker self-drop must not block or panic");
    assert!(weak.upgrade().is_none());
}

#[test]
fn final_owner_on_caller_thread_preserves_fifo_and_joins_loop() {
    let wake = ProcessWake::default();
    let (root_tx, root_rx) = signal::channel(1, &wake);
    let (control_tx, control_rx) = signal::channel(2, &wake);
    let stopped = Arc::new(AtomicBool::new(false));
    let thread_stopped = Arc::clone(&stopped);
    let options = ComponentBrokerOptions::default();
    let binding = binding();
    let mut state = LoopState::new(
        ProcessSpec::new("must-not-start-a-process"),
        binding.clone(),
        options,
        control_tx.clone(),
        wake,
    );
    let (done_tx, done_rx) = mpsc::channel();
    let thread = thread::spawn(move || {
        // Gate the real loop on receipt of Shutdown itself, rather than racing
        // Inspect processing against the caller's Drop. No sleeps or child spawn.
        let inspect = control_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(matches!(&inspect, ControlCommand::Inspect { .. }));
        let shutdown = control_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(matches!(&shutdown, ControlCommand::Shutdown));
        assert!(!thread_stopped.load(Ordering::Acquire));
        assert!(state.control_tx.try_send(inspect).is_ok());
        assert!(state.control_tx.try_send(shutdown).is_ok());
        state.run(root_rx, control_rx, &thread_stopped);
        done_tx.send(()).unwrap();
    });
    let broker = ComponentBroker {
        inner: Arc::new(BrokerInner {
            root_tx,
            control_tx,
            binding,
            options,
            stopped,
            thread: Mutex::new(Some(thread)),
        }),
    };
    let (ack, inspected) = mpsc::channel();
    assert!(
        broker
            .inner
            .control_tx
            .try_send(ControlCommand::Inspect { ack })
            .is_ok()
    );
    let (dropped_tx, dropped_rx) = mpsc::channel();
    thread::spawn(move || {
        drop(broker);
        done_rx
            .try_recv()
            .expect("off-thread drop must retain synchronous join");
        dropped_tx.send(()).unwrap();
    });
    dropped_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("off-thread drop must finish");
    assert_eq!(
        inspected
            .try_recv()
            .expect("queued controls precede external shutdown")
            .last_failure,
        None
    );
}

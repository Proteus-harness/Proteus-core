use std::{future::poll_fn, task::Poll};

use super::*;
use crate::v3::signal;

fn invocation(
    capacity: usize,
) -> (
    InvocationHandle,
    mpsc::Receiver<ControlCommand>,
    oneshot::Sender<InvocationTerminal>,
) {
    let wake = proteus_process_host::ProcessWake::default();
    let (control_tx, commands) = signal::channel(capacity, &wake);
    let (terminal_tx, terminal_rx) = oneshot::channel();
    let handle = InvocationHandle {
        invocation: InvocationRef {
            id: "h:3:7".to_owned(),
            generation: 3,
            target: ProcessComponentExportRef::new("context", "ack-probe"),
            root_id: "h:3:7".to_owned(),
            parent_id: None,
            depth: 0,
            deadline: Instant::now() + Duration::from_secs(10),
        },
        pid: 1,
        control_tx,
        terminal_rx: Some(TerminalReceiver::new(terminal_rx, wake)),
        notifications: None,
        dropped_notifications: Arc::new(AtomicU64::new(0)),
    };
    (handle, commands, terminal_tx)
}

async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}

fn cancel_ack(command: ControlCommand, expected_cause: CancelCause) -> CancelAck {
    let ControlCommand::Cancel {
        id,
        generation,
        cause,
        ack,
    } = command
    else {
        panic!("expected the admitted cancellation command");
    };
    assert_eq!(id, "h:3:7");
    assert_eq!(generation, 3);
    assert_eq!(cause, expected_cause);
    ack
}

fn assert_ack_missing(error: ComponentBrokerError) {
    assert_eq!(error.kind, ComponentBrokerErrorKind::Stopped);
    assert_eq!(error.message, "component broker did not acknowledge cancel");
}

#[tokio::test]
async fn async_ack_allows_current_thread_progress_without_consuming_terminal() {
    let (mut handle, commands, terminal_tx) = invocation(1);
    let cancel = handle.cancel_handle();
    let mut waiting = Box::pin(cancel.cancel_async(CancelCause::User));
    assert!(matches!(poll_once(waiting.as_mut()).await, Poll::Pending));

    // This task cannot run if awaiting ACK blocks the current-thread executor.
    let responder = tokio::spawn(async move {
        let ack = cancel_ack(commands.try_recv().unwrap(), CancelCause::User);
        assert!(ack.send(Ok(())));
    });
    waiting.await.unwrap();
    responder.await.unwrap();

    let mut terminal = Box::pin(handle.result());
    assert!(matches!(poll_once(terminal.as_mut()).await, Poll::Pending));
    terminal_tx
        .send(InvocationTerminal::ComponentLost(
            ComponentFailure::Protocol,
        ))
        .unwrap();
    assert_eq!(
        terminal.await.unwrap(),
        InvocationTerminal::ComponentLost(ComponentFailure::Protocol)
    );
}

#[test]
fn full_and_disconnected_errors_do_not_require_a_time_driver() {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(async {
            let (handle, commands, _terminal) = invocation(1);
            assert!(handle.control_tx.try_send(ControlCommand::Shutdown).is_ok());
            let cancel = handle.cancel_handle();
            let expected = ComponentBrokerError::new(
                ComponentBrokerErrorKind::Admission,
                "component broker control queue is full; cancel was not admitted",
            );
            assert_eq!(
                cancel.cancel_async(CancelCause::User).await.unwrap_err(),
                expected
            );
            assert_eq!(cancel.cancel(CancelCause::User).unwrap_err(), expected);
            assert!(matches!(
                commands.try_recv().unwrap(),
                ControlCommand::Shutdown
            ));

            drop(commands);
            let expected = ComponentBrokerError::stopped("component broker is stopped");
            assert_eq!(
                cancel.cancel_async(CancelCause::User).await.unwrap_err(),
                expected
            );
            assert_eq!(cancel.cancel(CancelCause::User).unwrap_err(), expected);
        });
}

#[tokio::test]
async fn broker_rejection_is_returned_unchanged() {
    let (handle, commands, _terminal) = invocation(1);
    let cancel = handle.cancel_handle();
    let mut waiting = Box::pin(cancel.cancel_async(CancelCause::Timeout));
    assert!(matches!(poll_once(waiting.as_mut()).await, Poll::Pending));
    let ack = cancel_ack(commands.try_recv().unwrap(), CancelCause::Timeout);
    let expected = ComponentBrokerError::new(
        ComponentBrokerErrorKind::ParentInactive,
        "invocation h:3:7 belongs to stale generation 3, current is 4",
    );
    assert!(ack.send(Err(expected.clone())));
    assert_eq!(waiting.await.unwrap_err(), expected);
}

#[tokio::test]
async fn closed_ack_sender_keeps_the_existing_stopped_error() {
    let (handle, commands, _terminal) = invocation(1);
    let cancel = handle.cancel_handle();
    let mut waiting = Box::pin(cancel.cancel_async(CancelCause::Shutdown));
    assert!(matches!(poll_once(waiting.as_mut()).await, Poll::Pending));
    drop(cancel_ack(
        commands.try_recv().unwrap(),
        CancelCause::Shutdown,
    ));
    assert_ack_missing(waiting.await.unwrap_err());
}

#[test]
fn dropping_unpolled_cancel_does_not_enqueue() {
    let (handle, commands, _terminal) = invocation(1);
    let cancel = handle.cancel_handle();
    drop(cancel.cancel_async(CancelCause::User));
    assert!(matches!(
        commands.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn dropping_ack_wait_does_not_retract_command_or_drop_terminal() {
    let (mut handle, commands, terminal_tx) = invocation(1);
    let cancel = handle.cancel_handle();
    let mut waiting = Box::pin(cancel.cancel_async(CancelCause::User));
    assert!(matches!(poll_once(waiting.as_mut()).await, Poll::Pending));
    drop(waiting);

    // The command is still delivered, even though its ACK receiver is gone.
    let ack = cancel_ack(commands.try_recv().unwrap(), CancelCause::User);
    assert!(!ack.send(Ok(())));
    terminal_tx.send(InvocationTerminal::Canceled).unwrap();
    assert_eq!(handle.result().await.unwrap(), InvocationTerminal::Canceled);
}

#[tokio::test]
async fn ack_timeout_does_not_retract_the_admitted_command() {
    let (handle, commands, _terminal) = invocation(1);
    let cancel = handle.cancel_handle();
    let error = tokio::time::timeout(
        CONTROL_ACK_TIMEOUT + Duration::from_secs(1),
        cancel.cancel_async(CancelCause::User),
    )
    .await
    .expect("the ACK wait must retain its bounded deadline")
    .unwrap_err();
    assert_ack_missing(error);
    let ack = cancel_ack(commands.try_recv().unwrap(), CancelCause::User);
    assert!(!ack.send(Ok(())));
}

#[tokio::test]
async fn observed_ready_ack_is_not_replaced_by_an_elapsed_timer() {
    let (sender, receiver) = oneshot::channel();
    let mut waiting = Box::pin(wait_for_cancel_ack(receiver));
    assert!(matches!(poll_once(waiting.as_mut()).await, Poll::Pending));
    // Do not poll the waiter while its timer expires. If ACK is ready on the
    // next poll, retain the received broker result rather than invent timeout.
    tokio::time::sleep(CONTROL_ACK_TIMEOUT + Duration::from_millis(20)).await;
    let expected = ComponentBrokerError::new(
        ComponentBrokerErrorKind::ParentInactive,
        "invocation h:3:7 is not active",
    );
    sender.send(Err(expected.clone())).unwrap();
    assert_eq!(waiting.await.unwrap_err(), expected);
}

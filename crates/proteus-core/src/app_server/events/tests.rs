use super::*;
use crate::{
    contracts::EventSink,
    domain::{Event, EventContext, EventEnvelope, new_message_id, new_thread_id, new_turn_id},
};

#[cfg(unix)]
mod active_followup;

#[tokio::test]
async fn failed_prestart_keeps_validated_prefix_and_ephemeral_input_until_retry_or_clear() {
    use crate::model_standard::{CanonicalMessage, ContentPart, MessageRole};
    use proteus_contracts::app_protocol::{AppExecutionState, AppRun, AppRunStatus};

    for projection_failed in [true, false] {
        let (events, _) = test_event_channel(8);
        let saved = crate::app_server::transcript_messages(&[
            crate::model_standard::CanonicalMessage::text(
                crate::model_standard::MessageRole::User,
                "saved history",
            ),
        ]);
        events.finish_progress(Ok(saved.into()));
        let prefix = events.completed_history().unwrap();
        let _ = events.send(AppServerEvent::UserMessageSubmitted {
            text: "accepted before preparation".into(),
            images: vec![crate::domain::ImageRef {
                id: "accepted-image".into(),
                name: "synthetic.png".into(),
                mime_type: "image/png".into(),
                path: "images/synthetic.png".into(),
            }],
        });
        let before = events.session_snapshot().unwrap();
        let queued_one = CanonicalMessage::text(MessageRole::User, "queued first");
        let queued_two = CanonicalMessage::new(
            MessageRole::User,
            vec![
                ContentPart::Text {
                    text: "queued second".into(),
                },
                ContentPart::Image {
                    image: crate::domain::ImageRef {
                        id: "queued-image".into(),
                        name: "queued.png".into(),
                        mime_type: "image/png".into(),
                        path: "images/queued.png".into(),
                    },
                },
                ContentPart::Text {
                    text: "second text part".into(),
                },
            ],
        );
        let queued_ids = [queued_one.id, queued_two.id];
        assert!(events.has_unstarted_input());
        events.finish_run_progress(
            if projection_failed {
                Err(anyhow::anyhow!("synthetic corrupt journal"))
            } else {
                // A canceled or locked execution may still allow a valid read.
                Ok(Vec::new().into())
            },
            vec![queued_one, queued_two],
        );
        let _ = events.send(AppServerEvent::ExecutionUpdated {
            execution: AppExecutionState {
                active: None,
                last: Some(AppRun {
                    run_id: "preparation".into(),
                    options: Default::default(),
                    status: AppRunStatus::Error,
                    error: Some("preparation failed before execution".into()),
                }),
            },
        });
        events.publish_snapshot().unwrap();
        let mut reopened = events.subscribe_session();
        assert!(matches!(
            reopened.recv().await.unwrap(),
            AppServerEvent::PendingRequestsUpdated { .. }
        ));
        let AppServerEvent::SessionSnapshot { snapshot } = reopened.recv().await.unwrap() else {
            panic!("preparation failure must remain readable on reconnect")
        };
        assert_eq!(
            serde_json::to_value(&snapshot.transcript[..2]).unwrap(),
            serde_json::to_value(&before.transcript).unwrap()
        );
        assert!(Arc::ptr_eq(&events.completed_history().unwrap(), &prefix));
        assert_eq!(snapshot.transcript.len(), 4);
        assert_eq!(snapshot.transcript[1].text, "accepted before preparation");
        assert_eq!(snapshot.transcript[1].message_id, None);
        assert_eq!(snapshot.transcript[1].images[0].id, "accepted-image");
        assert_eq!(snapshot.transcript[2].text, "queued first");
        assert_eq!(snapshot.transcript[2].message_id, Some(queued_ids[0]));
        assert_eq!(
            snapshot.transcript[3].text,
            "queued second\nsecond text part"
        );
        assert_eq!(snapshot.transcript[3].message_id, Some(queued_ids[1]));
        assert_eq!(snapshot.transcript[3].images[0].id, "queued-image");
        assert!(snapshot.transcript.iter().all(|row| !row.streaming));
        assert_eq!(
            serde_json::to_value(&events.session_snapshot().unwrap().transcript).unwrap(),
            serde_json::to_value(&snapshot.transcript).unwrap(),
        );
        assert!(snapshot.execution.active.is_none());
        assert_eq!(snapshot.execution.last.unwrap().status, AppRunStatus::Error);
        let _ = events.send(AppServerEvent::UserMessageSubmitted {
            text: "retry".into(),
            images: Vec::new(),
        });
        let retried = events.session_snapshot().unwrap();
        assert_eq!(retried.transcript.len(), 2);
        assert_eq!(retried.transcript[0].text, "saved history");
        assert_eq!(retried.transcript[1].text, "retry");
        assert!(retried.transcript[1].images.is_empty());
        events.finish_run_progress(
            Err(anyhow::anyhow!("retry preparation failed")),
            vec![CanonicalMessage::text(MessageRole::User, "retry queued")],
        );
        assert_eq!(events.session_snapshot().unwrap().transcript.len(), 3);
        events.finish_progress(Ok(Vec::new().into()));
        assert!(!events.has_unstarted_input());
        assert!(events.session_snapshot().unwrap().transcript.is_empty());
    }
}

#[test]
fn late_child_refresh_cannot_publish_a_new_active_turn_as_completed_history() {
    use proteus_contracts::app_protocol::{AppExecutionState, AppRun, AppRunStatus};
    let (events, _) = test_event_channel(8);
    let baseline = events.completed_history().unwrap();
    let _ = events.send(AppServerEvent::ExecutionUpdated {
        execution: AppExecutionState {
            active: Some(AppRun {
                run_id: "new-parent-turn".into(),
                options: Default::default(),
                status: AppRunStatus::Running,
                error: None,
            }),
            last: None,
        },
    });
    // A blocking late-binding projection may observe a turn admitted after
    // it was scheduled. Its partial rows must stay out of completed history.
    let partial =
        crate::app_server::transcript_messages(&[crate::model_standard::CanonicalMessage::text(
            crate::model_standard::MessageRole::Assistant,
            "still streaming",
        )]);
    events
        .refresh_idle_history(&baseline, partial.into())
        .unwrap();
    assert!(Arc::ptr_eq(&events.completed_history().unwrap(), &baseline));
    assert!(events.session_snapshot().unwrap().transcript.is_empty());
}

#[tokio::test]
async fn subscription_snapshot_covers_buffered_events_and_recovers_after_overflow() {
    let (events, _) = test_event_channel(2);
    let sink = RuntimeEventSink::default();
    assert!(sink.0.set(events.clone()).is_ok());
    let context = EventContext::new(
        events.pending_snapshot().session_id,
        new_thread_id(),
        Some(new_turn_id()),
    );
    let message_id = new_message_id();
    let mut subscription = events.subscribe_session();
    sink.append(EventEnvelope::new(
        context.clone(),
        1,
        Event::TurnStarted {
            session_id: context.session_id,
            thread_id: context.thread_id,
            turn_id: context.turn_id.unwrap(),
        },
    ))
    .await
    .unwrap();
    for seq in 2..12 {
        sink.append(EventEnvelope::new(
            context.clone(),
            seq,
            Event::AssistantTextDelta {
                message_id,
                phase: None,
                text: "x".into(),
                offset: (seq - 2) as usize,
            },
        ))
        .await
        .unwrap();
    }
    assert!(matches!(
        subscription.recv().await.unwrap(),
        AppServerEvent::PendingRequestsUpdated { .. }
    ));
    let AppServerEvent::SessionSnapshot { snapshot } = subscription.recv().await.unwrap() else {
        panic!("baseline")
    };
    assert_eq!(snapshot.transcript[0].text, "xxxxxxxxxx");
    assert_eq!(snapshot.seq, 11);
    assert_eq!(snapshot.root_thread_id, Some(context.thread_id));
    // The ring overflowed before sampling. Its retained deltas are already in
    // the baseline and must never be applied to its text a second time.
    let AppServerEvent::EventStreamLagged { .. } = subscription.recv().await.unwrap() else {
        panic!("lag")
    };
    assert!(matches!(
        subscription.recv().await.unwrap(),
        AppServerEvent::SessionSnapshot { .. }
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), subscription.recv())
            .await
            .is_err()
    );
    sink.append(EventEnvelope::new(
        context.clone(),
        12,
        Event::AssistantTextDelta {
            message_id,
            phase: None,
            text: "y".into(),
            offset: 10,
        },
    ))
    .await
    .unwrap();
    let AppServerEvent::Runtime { envelope } = subscription.recv().await.unwrap() else {
        panic!("new delta")
    };
    assert!(matches!(envelope.event, Event::AssistantTextDelta { ref text, .. } if text == "y"));
    // The authoritative view has no lossy forwarder ahead of it.
    assert_eq!(
        events.session_snapshot().unwrap().transcript[0].text,
        "xxxxxxxxxxy"
    );
}

#[tokio::test]
async fn reconnect_snapshot_replaces_live_progress_with_committed_transcript_once() {
    let (events, _) = test_event_channel(8);
    let stale_refresh = events.completed_history().unwrap();
    let sink = RuntimeEventSink::default();
    assert!(sink.0.set(events.clone()).is_ok());
    let context = EventContext::new(
        events.pending_snapshot().session_id,
        new_thread_id(),
        Some(new_turn_id()),
    );
    let mut subscription = events.subscribe_session();
    sink.append(EventEnvelope::new(
        context.clone(),
        1,
        Event::TurnStarted {
            session_id: context.session_id,
            thread_id: context.thread_id,
            turn_id: context.turn_id.unwrap(),
        },
    ))
    .await
    .unwrap();
    sink.append(EventEnvelope::new(
        context,
        2,
        Event::AssistantTextDelta {
            message_id: new_message_id(),
            phase: None,
            text: "committed".into(),
            offset: 0,
        },
    ))
    .await
    .unwrap();
    let committed =
        crate::app_server::transcript_messages(&[crate::model_standard::CanonicalMessage::text(
            crate::model_standard::MessageRole::Assistant,
            "committed",
        )]);
    events.finish_progress(Ok(committed.into()));
    events.publish_snapshot().unwrap();
    events
        .refresh_history(&stale_refresh, Vec::new().into())
        .unwrap();

    assert!(matches!(
        subscription.recv().await.unwrap(),
        AppServerEvent::PendingRequestsUpdated { .. }
    ));
    let AppServerEvent::SessionSnapshot { snapshot } = subscription.recv().await.unwrap() else {
        panic!("baseline")
    };
    assert_eq!(snapshot.seq, 4);
    assert_eq!(snapshot.transcript.len(), 1);
    assert_eq!(snapshot.transcript[0].text, "committed");
    // Every buffered live delta and the published snapshot are covered by the
    // baseline watermark, including the transition to completed history.
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), subscription.recv())
            .await
            .is_err()
    );
}

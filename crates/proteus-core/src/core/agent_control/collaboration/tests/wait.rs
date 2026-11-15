use super::*;

fn queue_completion(control: &CollaborationControl, host: Arc<TestHost>, task: &str) {
    let reservation = control.reserve(host.session_id, task, "explore").unwrap();
    let handle = AgentControlHandle::new(new_call_id(), "explore", new_thread_id());
    control
        .attach(
            host.session_id,
            &reservation.path,
            reservation.generation,
            handle,
            host.clone(),
        )
        .unwrap();
    control.complete(
        host.session_id,
        &reservation.path,
        reservation.generation,
        Ok(AgentControlResult::new(
            task,
            AgentLifecycleStatus::Completed,
        )),
    );
}

#[tokio::test]
async fn competing_waiter_keeps_waiting_after_another_consumer_takes_the_update() {
    let control = CollaborationControl::default();
    let host = Arc::new(TestHost::new(new_session_id()));
    let wait = WaitAgentTool::new(30_000, control.clone());
    let ctx = context(host.clone());
    let first_call = call("wait_agent", json!({"timeout_ms":30_000}));
    let second_call = call("wait_agent", json!({"timeout_ms":30_000}));
    let mut first = Box::pin(wait.invoke(&first_call, ctx.clone()));
    let mut second = Box::pin(wait.invoke(&second_call, ctx.clone()));
    assert!(futures_util::poll!(first.as_mut()).is_pending());
    assert!(futures_util::poll!(second.as_mut()).is_pending());

    queue_completion(&control, host.clone(), "first");
    let first_result = output(&first.await.unwrap());
    assert_eq!(first_result["timed_out"], false);
    assert_eq!(first_result["agents"].as_array().unwrap().len(), 1);
    assert_eq!(first_result["agents"][0]["path"], "/root/first");
    // Both waiters received the broadcast; the second no longer owns a record.
    assert!(futures_util::poll!(second.as_mut()).is_pending());

    queue_completion(&control, host, "second");
    let second_result = output(&second.await.unwrap());
    assert_eq!(second_result["timed_out"], false);
    assert_eq!(second_result["agents"].as_array().unwrap().len(), 1);
    assert_eq!(second_result["agents"][0]["path"], "/root/second");
    let empty = wait
        .invoke(&call("wait_agent", json!({"timeout_ms":0})), ctx)
        .await
        .unwrap();
    assert_eq!(output(&empty)["timed_out"], true);
}

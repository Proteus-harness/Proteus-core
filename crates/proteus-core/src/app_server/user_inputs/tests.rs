//! Queue identity belongs to the app-server, not to a local tool call.
use super::*;
use crate::{contracts::UserInputAnswer, domain::new_call_id};

struct Queue {
    send: mpsc::Sender<PendingUserInput>,
    receive: tokio::sync::broadcast::Receiver<AppServerEvent>,
    pending: PendingUserInputResponders,
    events: AppEventPublisher,
}

impl Queue {
    fn new() -> Self {
        let (send, receive) = mpsc::channel(8);
        let (events, _) = crate::app_server::events::test_event_channel(32);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        spawn_user_input_forwarder(receive, events.clone(), pending.clone(), Duration::ZERO);
        Self {
            send,
            receive: events.subscribe(),
            pending,
            events,
        }
    }

    async fn request(
        &mut self,
        label: &str,
    ) -> (UserInputRequest, oneshot::Receiver<UserInputResponse>) {
        let (responder, receiver) = oneshot::channel();
        self.send
            .send(PendingUserInput {
                request: UserInputRequest::new("call_0", "/workspace".into(), vec![])
                    .with_title(label),
                responder,
            })
            .await
            .unwrap();
        loop {
            if let AppServerEvent::UserInputRequested { request } =
                tokio::time::timeout(Duration::from_secs(2), self.receive.recv())
                    .await
                    .unwrap()
                    .unwrap()
            {
                assert_eq!(request.title.as_deref(), Some(label));
                return (*request, receiver);
            }
        }
    }

    async fn resolve(&self, id: &str, value: &str) -> Result<()> {
        resolve_pending_user_input(
            &self.pending,
            &self.events,
            id,
            UserInputResponse::new(HashMap::from([(
                "answer".into(),
                UserInputAnswer::new(vec![value.into()]),
            )])),
        )
        .await
    }
}

#[tokio::test]
async fn user_input_identity_is_unique_for_concurrent_local_call_ids() {
    let mut queue = Queue::new();
    let (first, first_response) = queue.request("first").await;
    let (second, second_response) = queue.request("second").await;
    assert_ne!(first.request_id, second.request_id);
    assert_ne!(first.request_id, "call_0");
    assert_eq!(first.seq, 1);
    assert_eq!(second.seq, 2);
    assert_eq!(queue.pending.lock().await.len(), 2);
    queue
        .resolve(&second.request_id, "second only")
        .await
        .unwrap();
    queue
        .resolve(&first.request_id, "first only")
        .await
        .unwrap();
    assert_eq!(
        first_response.await.unwrap().answers["answer"].answers,
        ["first only"]
    );
    assert_eq!(
        second_response.await.unwrap().answers["answer"].answers,
        ["second only"]
    );
    assert!(queue.pending.lock().await.is_empty());
}

#[tokio::test]
async fn user_input_identity_does_not_reuse_a_completed_generation() {
    let mut queue = Queue::new();
    let (first, first_response) = queue.request("old generation").await;
    queue
        .resolve(&first.request_id, "old result")
        .await
        .unwrap();
    first_response.await.unwrap();
    let (second, mut second_response) = queue.request("new generation").await;
    // This is also the callback used by an already-scheduled old timeout.
    // A delayed client response or timeout cannot close the replacement.
    assert!(
        queue
            .resolve(&first.request_id, "stale result")
            .await
            .is_err()
    );
    assert_eq!(queue.pending.lock().await.len(), 1);
    assert!(matches!(
        second_response.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert!(queue.resolve(&new_call_id(), "unknown").await.is_err());
    queue
        .resolve(&second.request_id, "new result")
        .await
        .unwrap();
    assert_eq!(
        second_response.await.unwrap().answers["answer"].answers,
        ["new result"]
    );
}

#[tokio::test]
async fn user_input_identity_cancellation_preserves_same_id_sibling() {
    let mut queue = Queue::new();
    let (first, first_response) = queue.request("cancelled").await;
    let (second, second_response) = queue.request("survivor").await;
    drop(first_response);
    let resolved = tokio::time::timeout(Duration::from_secs(2), queue.receive.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(resolved, AppServerEvent::UserInputResolved { request_id } if request_id == first.request_id)
    );
    let pending = queue.pending.lock().await;
    assert!(!pending.contains_key(&first.request_id));
    assert!(pending.contains_key(&second.request_id));
    drop(pending);
    queue.resolve(&second.request_id, "survived").await.unwrap();
    assert_eq!(
        second_response.await.unwrap().answers["answer"].answers,
        ["survived"]
    );
}

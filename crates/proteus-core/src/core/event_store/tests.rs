use super::*;
use crate::domain::{EventContext, new_session_id, new_thread_id};

struct Recorder {
    index: usize,
    fail: bool,
    deliveries: Arc<StdMutex<Vec<(usize, EventEnvelope)>>>,
}

#[async_trait::async_trait]
impl EventSink for Recorder {
    async fn append(&self, envelope: EventEnvelope) -> Result<()> {
        self.deliveries.lock().unwrap().push((self.index, envelope));
        anyhow::ensure!(!self.fail, "sink {}", self.index);
        Ok(())
    }
}

#[tokio::test]
async fn fanout_delivers_each_envelope_in_order_and_reports_only_the_first_failure() {
    let envelope = EventEnvelope::new(
        EventContext::new(new_session_id(), new_thread_id(), None),
        7,
        Event::AssistantReasoningDelta { text: "δ".into() },
    );
    for count in 0..=4 {
        for mask in 0..1 << count {
            let deliveries = Arc::new(StdMutex::new(Vec::new()));
            let sinks = (0..count)
                .map(|index| {
                    Arc::new(Recorder {
                        index,
                        fail: mask & (1 << index) != 0,
                        deliveries: deliveries.clone(),
                    }) as Arc<dyn EventSink>
                })
                .collect();
            let result = FanoutEventSink::new(sinks).append(envelope.clone()).await;
            let expected_error = (0..count)
                .find(|index| mask & (1 << index) != 0)
                .map(|index| format!("sink {index}"));
            assert_eq!(result.err().map(|e| e.to_string()), expected_error);
            let actual = deliveries.lock().unwrap();
            assert_eq!(actual.len(), count);
            for (index, (sink, event)) in actual.iter().enumerate() {
                assert_eq!(*sink, index);
                assert_eq!(event, &envelope);
            }
        }
    }
}

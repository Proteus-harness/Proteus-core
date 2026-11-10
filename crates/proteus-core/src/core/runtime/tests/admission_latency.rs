//! Opt-in synthetic pre-echo boundary probe; no model or user data is involved.
use super::*;
use crate::{
    core::SessionStore,
    domain::{new_session_id, new_thread_id},
};
use std::time::Instant;

#[tokio::test]
#[ignore = "synthetic latency probe; run explicitly with --ignored --nocapture"]
async fn synthetic_pre_echo_boundaries() {
    for count in [100_usize, 1_000, 5_000] {
        let root = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path(), workspace.path(), new_session_id()).unwrap();
        let thread = new_thread_id();
        for start in (0..count).step_by(32) {
            let messages = (start..count.min(start + 32))
                .map(|index| {
                    CanonicalMessage::text(
                        if index % 2 == 0 {
                            MessageRole::User
                        } else {
                            MessageRole::Assistant
                        },
                        format!("synthetic {index}: {}", "x".repeat(8_192)),
                    )
                })
                .collect::<Vec<_>>();
            store.append_history(thread, None, &messages).await.unwrap();
        }
        let journal_bytes = std::fs::metadata(store.journal_path()).unwrap().len();
        store.hibernate_writer().await;
        let runtime = AgentRuntime::builder(AppConfig::default(), workspace.path().to_owned())
            .with_config_path(Some(&root.path().join("config.toml")))
            .with_module_catalog(test_catalog())
            .resume_from_session_view(store.clone(), count.div_ceil(32) as u64, count, thread)
            .build()
            .unwrap();
        for sample in 0..3 {
            for mode in ["cold", "warm", "active_queue"] {
                runtime.session.steering.abort().await;
                if mode == "cold" {
                    runtime.session.release_history().await;
                } else if mode == "active_queue" {
                    runtime
                        .reserve_user_message("active root".into())
                        .await
                        .unwrap();
                }
                let total = Instant::now();
                let stage = Instant::now();
                let message = runtime
                    .prepare_user_message("next input", &Default::default())
                    .await
                    .unwrap();
                let input_ms = stage.elapsed().as_secs_f64() * 1_000.0;
                let stage = Instant::now();
                let reservation = runtime
                    .try_reserve_prepared_user_message(message, Default::default())
                    .await
                    .unwrap();
                let snapshot_reservation_ms = stage.elapsed().as_secs_f64() * 1_000.0;
                assert!(matches!(
                    reservation,
                    UserMessageReservationAttempt::Ready(_)
                ));
                let pre_echo_ms = total.elapsed().as_secs_f64() * 1_000.0;
                // This is the same strict work executed by the spawned turn,
                // now outside app-server's acceptance/echo boundary.
                let stage = Instant::now();
                runtime.validate_cached_resume().await.unwrap();
                let execution_resume_ms = stage.elapsed().as_secs_f64() * 1_000.0;
                eprintln!(
                    "ADMISSION_LATENCY {}",
                    serde_json::json!({
                        "messages": count, "text_bytes": count * 8_192, "journal_bytes": journal_bytes, "sample": sample, "mode": mode,
                        "execution_resume_ms": execution_resume_ms, "input_ms": input_ms,
                        "snapshot_reservation_ms": snapshot_reservation_ms,
                        "pre_echo_ms": pre_echo_ms,
                        "total_ms": total.elapsed().as_secs_f64() * 1_000.0,
                    })
                );
            }
        }
        runtime.session.steering.abort().await;
    }
}

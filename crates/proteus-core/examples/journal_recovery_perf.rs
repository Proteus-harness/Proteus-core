//! Isolated synthetic cold-journal probe; no modules, models or user data.
use anyhow::{Result, ensure};
use proteus_contracts::{
    domain::{ThreadId, new_session_id, new_thread_id},
    model_standard::{CanonicalMessage, MessageRole},
};
use proteus_core::core::SessionStore;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
struct Fixture {
    session: PathBuf,
    thread: ThreadId,
    snapshots: usize,
    message: CanonicalMessage,
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() >= 3,
        "journal_recovery_perf seed ROOT RECORDS KIB | recover/projection/verify ROOT"
    );
    let root = PathBuf::from(&args[2]);
    if args[1] == "seed" {
        ensure!(
            args.len() == 5 && !root.exists(),
            "seed requires a fresh output root"
        );
        let snapshots: usize = args[3].parse()?;
        let kib: usize = args[4].parse()?;
        ensure!((1..=1024).contains(&snapshots) && (1..=1024).contains(&kib));
        std::fs::create_dir_all(root.join("workspace"))?;
        let store = SessionStore::new(
            &root.join("config"),
            &root.join("workspace"),
            new_session_id(),
        )?;
        let fixture = Fixture {
            session: store
                .session_dir()
                .strip_prefix(std::fs::canonicalize(&root)?)?
                .to_owned(),
            thread: new_thread_id(),
            snapshots,
            message: CanonicalMessage::text(MessageRole::User, "x".repeat(kib * 1024)),
        };
        for _ in 0..snapshots {
            store
                .replace_history(
                    fixture.thread,
                    None,
                    std::slice::from_ref(&fixture.message),
                    None,
                )
                .await?;
        }
        std::fs::write(root.join("fixture.json"), serde_json::to_vec(&fixture)?)?;
        println!(
            "{}",
            serde_json::json!({"session":fixture.session,"snapshots":snapshots,"text_kib":kib})
        );
        return Ok(());
    }
    let mut fixture: Fixture = serde_json::from_slice(&std::fs::read(root.join("fixture.json"))?)?;
    // Copies retain the encoded workspace identity but have their own writer lease.
    fixture.session = root.join(&fixture.session);
    let store = SessionStore::open(fixture.session.clone())?;
    let started = Instant::now();
    match args[1].as_str() {
        "recover" => {
            let continuation = CanonicalMessage::text(MessageRole::Assistant, "continued");
            store
                .append_history(fixture.thread, None, std::slice::from_ref(&continuation))
                .await?;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.;
            std::fs::write(
                root.join("continuation.json"),
                serde_json::to_vec(&continuation)?,
            )?;
            println!(
                "{}",
                serde_json::json!({"mode":"recover","elapsed_ms":elapsed_ms})
            );
            // Keep the initialized writer alive for settled PSS sampling. Strict
            // verification runs in another process, outside timing/RSS evidence.
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        "projection" => {
            let projection = store.load_projection()?;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.;
            ensure!(projection.records.len() == fixture.snapshots);
            ensure!(projection.history == [fixture.message]);
            println!(
                "{}",
                serde_json::json!({"mode":"projection","elapsed_ms":elapsed_ms})
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
            std::hint::black_box(projection);
        }
        "verify" => {
            let mut expected = vec![fixture.message];
            let appended = root.join("continuation.json");
            let count = if appended.exists() {
                expected.push(serde_json::from_slice(&std::fs::read(appended)?)?);
                fixture.snapshots + 1
            } else {
                fixture.snapshots
            };
            let projection = store.load_projection()?;
            ensure!(projection.records.len() == count);
            ensure!(projection.history_revision == count as u64);
            ensure!(
                projection.history == expected,
                "canonical identities/content changed"
            );
            ensure!(
                projection
                    .records
                    .iter()
                    .enumerate()
                    .all(|(i, r)| r.session_seq == i as u64 + 1)
            );
            println!(
                "{}",
                serde_json::json!({"verified_records":count,"history_messages":expected.len()})
            );
        }
        _ => anyhow::bail!("unknown mode"),
    }
    Ok(())
}

//! Synthetic canonical history for native startup/RAM comparisons; no model invocation.
use anyhow::{Result, ensure};
use proteus_contracts::{
    domain::{new_session_id, new_thread_id},
    model_standard::{CanonicalMessage, MessageRole},
};
use proteus_core::core::SessionStore;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 5,
        "session_index_fixture CONFIG_ROOT WORKSPACE ROWS TEXT_REPEATS"
    );
    let root = PathBuf::from(&args[1]);
    let cwd = PathBuf::from(&args[2]);
    let rows: usize = args[3].parse()?;
    let repeats: usize = args[4].parse()?;
    ensure!(rows > 0 && repeats > 0);
    std::fs::create_dir_all(&cwd)?;
    let store = SessionStore::new(&root, &cwd, new_session_id())?;
    let messages: Vec<_> = (0..rows)
        .map(|i| {
            CanonicalMessage::text(
                if i % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                format!(
                    "Fixture row {i}\n{}",
                    "Синтетический текст для замера памяти и прокрутки. ".repeat(repeats)
                ),
            )
        })
        .collect();
    store
        .append_history(new_thread_id(), None, &messages)
        .await?;
    println!(
        "{}",
        serde_json::json!({"session_dir":store.session_dir(),"rows":rows})
    );
    Ok(())
}

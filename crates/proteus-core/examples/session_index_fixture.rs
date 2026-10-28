//! Synthetic canonical history for native startup/RAM comparisons; no model invocation.
use anyhow::{Result, ensure};
use proteus_contracts::{
    domain::{HistoryCompactionReport, new_session_id, new_thread_id},
    model_standard::{CanonicalMessage, MessageRole},
};
use proteus_core::core::SessionStore;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 5 || args.len() == 6,
        "session_index_fixture CONFIG_ROOT WORKSPACE ROWS TEXT_REPEATS [MODEL_HISTORY_ROWS]"
    );
    let root = PathBuf::from(&args[1]);
    let cwd = PathBuf::from(&args[2]);
    let rows: usize = args[3].parse()?;
    let repeats: usize = args[4].parse()?;
    let model_history_rows: Option<usize> = args.get(5).map(|value| value.parse()).transpose()?;
    ensure!(rows > 0 && repeats > 0);
    ensure!(model_history_rows.is_none_or(|keep| keep > 0 && keep <= rows));
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
    let thread_id = new_thread_id();
    store.append_history(thread_id, None, &messages).await?;
    if let Some(keep) = model_history_rows {
        let tail = &messages[messages.len() - keep..];
        let mut report = HistoryCompactionReport::unchanged(
            messages.len(),
            Some("Synthetic benchmark: retain canonical tail; no summarization model".into()),
        );
        report.changed = true;
        report.output_messages = tail.len();
        report.summary_source = Some("history-scale-fixture-tail".into());
        store
            .replace_history(thread_id, None, tail, Some(report))
            .await?;
    }
    println!(
        "{}",
        serde_json::json!({"session_dir":store.session_dir(),"rows":rows,
            "model_history_rows":model_history_rows.unwrap_or(rows)})
    );
    Ok(())
}

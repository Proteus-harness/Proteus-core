//! Synthetic canonical history for native startup/RAM comparisons; no model invocation.
//! Optional detached model exchanges exercise cold journal payload hydration, not workflow replay.
use anyhow::{Result, ensure};
use proteus_contracts::{
    contracts::{ExecutionAttribution, ModelCallOrigin, OperationOwner},
    domain::{
        HistoryCompactionReport, ModelRef, new_exchange_id, new_execution_id, new_session_id,
        new_thread_id,
    },
    model_standard::{
        CanonicalMessage, CanonicalModelRequest, CanonicalModelResponse, ContentPart, FinishReason,
        MessageRole,
    },
};
use proteus_core::core::{
    JournalEntry, JournalProjection, ModelRequestRecorded, ModelResponseOutcome,
    ModelResponseRecorded, SessionStore,
};
use std::path::PathBuf;

const USAGE: &str = "session_index_fixture CONFIG_ROOT WORKSPACE ROWS TEXT_REPEATS \
    [MODEL_HISTORY_ROWS] [--model-request-repeats N] [--request-payload-kib K]";
const REQUEST_PAYLOAD_PREFIX: &str = "SYNTHETIC_REQUEST_PAYLOAD_";

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let flags_at = args
        .iter()
        .position(|value| value.starts_with("--"))
        .unwrap_or(args.len());
    let positional = &args[1..flags_at];
    ensure!(positional.len() == 4 || positional.len() == 5, "{USAGE}");
    let root = PathBuf::from(&positional[0]);
    let cwd = PathBuf::from(&positional[1]);
    let rows: usize = positional[2].parse()?;
    let repeats: usize = positional[3].parse()?;
    let model_history_rows: Option<usize> =
        positional.get(4).map(|value| value.parse()).transpose()?;
    let mut model_request_repeats = 0_usize;
    let mut request_payload_kib = 0_usize;
    let mut flags = args[flags_at..].chunks_exact(2);
    for flag in &mut flags {
        match flag[0].as_str() {
            "--model-request-repeats" => model_request_repeats = flag[1].parse()?,
            "--request-payload-kib" => request_payload_kib = flag[1].parse()?,
            other => anyhow::bail!("unknown flag {other}; {USAGE}"),
        }
    }
    ensure!(
        flags.remainder().is_empty(),
        "flag requires a value; {USAGE}"
    );
    ensure!(
        model_request_repeats <= 128,
        "model request repeats must be 0..=128"
    );
    ensure!(
        request_payload_kib <= 8192,
        "request payload must be 0..=8192 KiB"
    );
    let request_payload_bytes = request_payload_kib * 1024;
    let total_request_payload_bytes = model_request_repeats * request_payload_bytes;
    ensure!(
        total_request_payload_bytes <= 1024 * 1024 * 1024,
        "total request payload exceeds 1 GiB"
    );
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
    let active_history = &messages[messages.len() - model_history_rows.unwrap_or(rows)..];
    let request_record_bytes = seed_model_requests(
        &store,
        active_history,
        model_request_repeats,
        request_payload_bytes,
    )
    .await?;

    // Hydrate through the public reader, then run strict canonical validation.
    // No raw JSON journal writes or replacements stand in for a valid lifecycle.
    let records = store.load_records()?;
    let mut observed_requests = 0_usize;
    let mut observed_responses = 0_usize;
    let mut observed_payload_bytes = 0_usize;
    for record in &records {
        match &record.entry {
            JournalEntry::ModelRequestRecorded(request) => {
                observed_requests += 1;
                let extra = request
                    .request
                    .messages
                    .last()
                    .expect("synthetic system message");
                ensure!(
                    extra.role == MessageRole::System,
                    "request payload role changed"
                );
                let [part] = extra.parts.as_slice() else {
                    anyhow::bail!("synthetic payload must contain one text part");
                };
                let ContentPart::Text { text } = &part.payload else {
                    anyhow::bail!("synthetic payload must be text");
                };
                ensure!(
                    text.len() == request_payload_bytes,
                    "request payload byte count changed"
                );
                observed_payload_bytes += text.len();
            }
            JournalEntry::ModelResponseRecorded(_) => observed_responses += 1,
            _ => {}
        }
    }
    ensure!(
        observed_requests == model_request_repeats && observed_responses == model_request_repeats
    );
    ensure!(observed_payload_bytes == total_request_payload_bytes);
    let projection = JournalProjection::build(store.session_id(), records)?;
    ensure!(
        projection.history == active_history,
        "canonical history changed while seeding model exchanges"
    );
    ensure!(projection.interrupted_model_exchanges.is_empty());
    ensure!(projection.unsettled_turns.is_empty());
    println!(
        "{}",
        serde_json::json!({"session_dir":store.session_dir(),"rows":rows,
            "model_history_rows":model_history_rows.unwrap_or(rows),
            "model_request_repeats": observed_requests, "model_response_count": observed_responses,
            "request_payload_kib": request_payload_kib,
            "request_payload_bytes_per_request": request_payload_bytes,
            "total_request_payload_bytes": observed_payload_bytes,
            "request_record_bytes_per_request": request_record_bytes,
            "total_request_record_bytes": request_record_bytes * observed_requests,
            "canonical_validation": "passed", "workflow_replay_verified": false})
    );
    Ok(())
}

async fn seed_model_requests(
    store: &SessionStore,
    active_history: &[CanonicalMessage],
    repeats: usize,
    payload_bytes: usize,
) -> Result<usize> {
    if repeats == 0 {
        return Ok(0);
    }
    let mut payload = REQUEST_PAYLOAD_PREFIX.to_owned();
    payload.truncate(payload_bytes);
    payload.extend(std::iter::repeat_n(
        'x',
        payload_bytes.saturating_sub(payload.len()),
    ));
    // Preserve all existing message/part IDs. Repeated requests share the same
    // synthetic system message identity/content, but use unique exchange IDs.
    let mut request_messages = active_history.to_vec();
    request_messages.push(CanonicalMessage::text(MessageRole::System, payload));
    let request = CanonicalModelRequest::new(
        ModelRef::new("fixture", "journal-payload"),
        request_messages,
    );
    let mut request_record_bytes = 0;
    for index in 0..repeats {
        let exchange_id = new_exchange_id();
        let recorded = ModelRequestRecorded {
            exchange_id,
            origin: ModelCallOrigin::Auxiliary,
            owner: OperationOwner::Workflow,
            request: request.clone(),
        };
        if index == 0 {
            request_record_bytes = serde_json::to_vec(&recorded)?.len();
            ensure!(
                request_record_bytes <= 64 * 1024 * 1024,
                "model request exceeds the 64 MiB journal payload bound"
            );
        }
        // This is the typed detached model lifecycle used by the journal tests,
        // not a fabricated agent turn and not a call to a model implementation.
        let attribution = ExecutionAttribution::detached(new_execution_id());
        store
            .append_execution_journal_entry(
                attribution,
                JournalEntry::ModelRequestRecorded(recorded),
            )
            .await?;
        store
            .append_execution_journal_entry(
                attribution,
                JournalEntry::ModelResponseRecorded(ModelResponseRecorded {
                    exchange_id,
                    outcome: ModelResponseOutcome::Response {
                        response: CanonicalModelResponse::new(
                            CanonicalMessage::text(
                                MessageRole::Assistant,
                                "SYNTHETIC_REQUEST_RESPONSE",
                            ),
                            Vec::new(),
                            FinishReason::Stop,
                        ),
                    },
                }),
            )
            .await?;
    }
    Ok(request_record_bytes)
}

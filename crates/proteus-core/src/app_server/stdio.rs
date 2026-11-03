use std::path::PathBuf;

use anyhow::Result;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    sync::mpsc,
};

use crate::{
    contracts::CancellationToken,
    core::{AppConfig, SteeringQueueReceipt},
};

use super::runs::SendDispatch;
use super::{StdioOutput, StdioRequest};

mod session;

pub async fn run_stdio_app_server(
    config: AppConfig,
    cwd: PathBuf,
    config_path: Option<PathBuf>,
    resume_session_dir: Option<PathBuf>,
    fresh_session: bool,
    parent_session_id: Option<crate::domain::SessionId>,
) -> Result<()> {
    let original_config = config.clone();
    let original_cwd = cwd.clone();
    let mut server = if let Some(parent_session_id) = parent_session_id {
        anyhow::ensure!(
            fresh_session && resume_session_dir.is_none(),
            "parent session requires a fresh stdio session"
        );
        super::AgentAppServer::launch_child(config, cwd, config_path.as_deref(), parent_session_id)
            .await?
    } else {
        super::startup::launch_selected(
            config,
            cwd,
            config_path.as_deref(),
            resume_session_dir,
            fresh_session,
        )
        .await?
    };
    let (output_tx, mut output_rx) = mpsc::channel::<StdioOutput>(256);

    let (mut bridge, _) = session::start_bridge(&server, &output_tx).await?;

    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::BufWriter::new(tokio::io::stdout());
        while let Some(output) = output_rx.recv().await {
            let line = serde_json::to_string(&output)?;
            stdout.write_all(line.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
        Ok::<(), anyhow::Error>(())
    });

    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut shutdown_requested = false;
    // Command catalog/service preparation happens before RunRegistry admission.
    // Count the entire request so rotation cannot retire its runtime midway.
    let mut commands = tokio::task::JoinSet::new();

    while let Some(line) = lines.next_line().await? {
        while commands.try_join_next().is_some() {}
        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<StdioRequest>(&line) {
            Ok(request) => request,
            Err(error) => {
                send_stdio_response(
                    &output_tx,
                    None,
                    Err(anyhow::anyhow!("invalid JSONL request: {error}")),
                )
                .await;
                continue;
            }
        };
        let id = request.id();

        match request {
            StdioRequest::NewSession { .. } => {
                if !commands.is_empty() {
                    send_stdio_response(
                        &output_tx,
                        id,
                        Err(anyhow::anyhow!(
                            "new session requires an idle command stream"
                        )),
                    )
                    .await;
                    continue;
                }
                if let Err(error) = server.close_idle_runs().await {
                    send_stdio_response(&output_tx, id, Err(error)).await;
                    continue;
                }
                // Do not publish the retired runtime's Shutdown to the next
                // logical task. Release it before launching replacement modules.
                session::stop_bridge(bridge).await;
                server.shutdown().await;
                drop(server);
                let replacement = async {
                    let next = match parent_session_id {
                        Some(parent) => {
                            super::AgentAppServer::launch_child(
                                original_config.clone(),
                                original_cwd.clone(),
                                config_path.as_deref(),
                                parent,
                            )
                            .await?
                        }
                        None => {
                            super::AgentAppServer::launch(
                                original_config.clone(),
                                original_cwd.clone(),
                                config_path.as_deref(),
                            )
                            .await?
                        }
                    };
                    let (bridge, identity) = session::start_bridge(&next, &output_tx).await?;
                    Ok::<_, anyhow::Error>((next, bridge, identity))
                }
                .await;
                match replacement {
                    Ok((next, next_bridge, identity)) => {
                        server = next;
                        bridge = next_bridge;
                        send_stdio_response(
                            &output_tx,
                            id,
                            Ok(Some(serde_json::to_value(identity)?)),
                        )
                        .await;
                    }
                    Err(error) => {
                        // The old session is closed. No half-open connection is
                        // usable after a failed rotation.
                        send_stdio_response(
                            &output_tx,
                            id,
                            Err(anyhow::anyhow!("new session failed: {error:#}")),
                        )
                        .await;
                        drop(output_tx);
                        writer.await??;
                        return Err(error);
                    }
                }
            }
            StdioRequest::CommandCatalog { .. } => {
                let result = server
                    .command_catalog()
                    .await
                    .and_then(|v| Ok(Some(serde_json::to_value(v)?)));
                send_stdio_response(&output_tx, id, result).await;
            }
            StdioRequest::ExecuteCommand { text, .. } => {
                let server = server.clone();
                let tx = output_tx.clone();
                commands.spawn(async move {
                    let result = server
                        .execute_command(id.clone(), &text)
                        .await
                        .and_then(|v| Ok(Some(serde_json::to_value(v)?)));
                    send_stdio_response(&tx, id, result).await;
                });
            }
            StdioRequest::Send {
                id,
                text,
                images,
                options,
            } => {
                match server
                    .dispatch_user_message(
                        id.clone(),
                        crate::domain::UserMessageInput { text, images },
                        options,
                        CancellationToken::new(),
                    )
                    .await
                {
                    Ok(SendDispatch::Queued(receipt)) => {
                        send_stdio_response(&output_tx, id, Ok(Some(queued_response(&receipt))))
                            .await;
                    }
                    Ok(SendDispatch::Started(result)) => {
                        let tx = output_tx.clone();
                        tokio::spawn(async move {
                            let result = match result.await {
                                Ok(result) => result.and_then(|output| {
                                    serde_json::to_value(output).map(Some).map_err(Into::into)
                                }),
                                Err(error) => Err(error.into()),
                            };
                            send_stdio_response(&tx, id, result).await;
                        });
                    }
                    Err(error) => send_stdio_response(&output_tx, id, Err(error)).await,
                }
            }
            StdioRequest::EditQueuedMessage {
                message_id, text, ..
            } => {
                send_stdio_response(
                    &output_tx,
                    id,
                    server
                        .edit_queued_user_message(message_id, text)
                        .await
                        .map(|_| None),
                )
                .await;
            }
            StdioRequest::DeleteQueuedMessage { message_id, .. } => {
                send_stdio_response(
                    &output_tx,
                    id,
                    server
                        .delete_queued_user_message(message_id)
                        .await
                        .map(|_| None),
                )
                .await;
            }
            StdioRequest::ClearHistory { .. } => {
                send_stdio_response(&output_tx, id, server.clear_history().await.map(|_| None))
                    .await;
            }
            StdioRequest::UsageSummary { .. } => {
                let result = match server.usage_snapshot().await {
                    Ok(snapshot) => serde_json::to_value(snapshot).map(Some).map_err(Into::into),
                    Err(error) => Err(error),
                };
                send_stdio_response(&output_tx, id, result).await;
            }
            StdioRequest::HistorySummary { .. } => {
                let result = serde_json::to_value(server.history_summary().await)
                    .map(Some)
                    .map_err(anyhow::Error::from);
                send_stdio_response(&output_tx, id, result).await;
            }
            StdioRequest::Remember { kind, content, .. } => {
                let result = server.remember(kind, content).await.and_then(|result| {
                    serde_json::to_value(result)
                        .map(Some)
                        .map_err(anyhow::Error::from)
                });
                send_stdio_response(&output_tx, id, result).await;
            }
            StdioRequest::Approval {
                approval_id,
                approved,
                note,
                cache,
                ..
            } => {
                send_stdio_response(
                    &output_tx,
                    id,
                    server
                        .respond_approval(&approval_id, approved, note, cache)
                        .await
                        .map(|_| None),
                )
                .await;
            }
            StdioRequest::UserInput {
                request_id,
                response,
                ..
            } => {
                send_stdio_response(
                    &output_tx,
                    id,
                    server
                        .respond_user_input(&request_id, response)
                        .await
                        .map(|_| None),
                )
                .await;
            }
            StdioRequest::Cancel { target_id, .. } => {
                let result = server.cancel_run(&target_id).await;
                send_stdio_response(&output_tx, id, result.map(|_| None)).await;
            }
            StdioRequest::SetPermissionMode { mode, .. } => {
                server.set_permission_mode(mode).await;
                send_stdio_response(
                    &output_tx,
                    id,
                    Ok(Some(serde_json::json!({ "mode": mode }))),
                )
                .await;
            }
            StdioRequest::SetModel { model, .. } => {
                let result = server.set_model_name(model.clone()).await;
                send_stdio_response(
                    &output_tx,
                    id,
                    match result {
                        Ok(()) => Ok(Some(serde_json::json!({ "model": model, "config": server.config_summary().await }))),
                        Err(error) => Err(error),
                    },
                )
                .await;
            }
            StdioRequest::SetReasoningEffort { effort, .. } => {
                let result = server.set_reasoning_effort(effort.clone()).await;
                send_stdio_response(
                    &output_tx,
                    id,
                    result.map(|_| Some(serde_json::json!({ "effort": effort }))),
                )
                .await;
            }
            StdioRequest::SetReasoningEnabled { enabled, .. } => {
                let result = server.set_reasoning_enabled(enabled).await;
                send_stdio_response(
                    &output_tx,
                    id,
                    result.map(|_| Some(serde_json::json!({ "enabled": enabled }))),
                )
                .await;
            }
            StdioRequest::ConfigSummary { .. } => {
                send_stdio_response(&output_tx, id, Ok(Some(server.config_summary().await))).await;
            }
            StdioRequest::ReloadTools { .. } => {
                let result = server.reload_tools().await.and_then(|report| {
                    serde_json::to_value(report)
                        .map(Some)
                        .map_err(anyhow::Error::from)
                });
                send_stdio_response(&output_tx, id, result).await;
            }
            StdioRequest::Shutdown { .. } => {
                shutdown_requested = true;
                server.shutdown().await;
                send_stdio_response(&output_tx, id, Ok(None)).await;
                break;
            }
        }
    }

    if !shutdown_requested {
        server.shutdown().await;
    }
    // Keep EOF semantics: service requests admitted before disconnect finish
    // normally; runtime-owned module commands have already settled in shutdown.
    while commands.join_next().await.is_some() {}
    let _ = bridge.await;
    drop(output_tx);
    writer.await??;
    Ok(())
}

fn queued_response(receipt: &SteeringQueueReceipt) -> Value {
    serde_json::json!({
        "accepted": true,
        "queued": true,
        "message_id": receipt.message_id,
        "active_turn_id": receipt.active_turn_id,
        "queued_count": receipt.queued_count,
    })
}

async fn send_stdio_response(
    output_tx: &mpsc::Sender<StdioOutput>,
    id: Option<String>,
    result: Result<Option<Value>>,
) {
    let output = match result {
        Ok(output) => StdioOutput::Response {
            id,
            ok: true,
            output,
            error: None,
        },
        Err(error) => StdioOutput::Response {
            id,
            ok: false,
            output: None,
            error: Some(format!("{error:#}")),
        },
    };
    let _ = output_tx.send(output).await;
}

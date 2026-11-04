use super::{
    sessions::{server_for_query, server_for_session},
    state::HttpAppState,
    workspace::query_params,
};
use crate::contracts::{ToolTerminalAction, ToolTerminalResponse};
use anyhow::{Result, bail};
use serde::Deserialize;
use uuid::Uuid;

// No Debug: input bytes may contain credentials.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TerminalControlRequest {
    pub session_dir: std::path::PathBuf,
    pub id: Uuid,
    #[serde(default)]
    pub data_base64: Option<String>,
    #[serde(default)]
    pub columns: Option<u16>,
    #[serde(default)]
    pub rows: Option<u16>,
}
pub(super) async fn control(
    state: &HttpAppState,
    path: &str,
    request: TerminalControlRequest,
) -> Result<ToolTerminalResponse> {
    let action = match path {
        "/terminals/input" if request.columns.is_none() && request.rows.is_none() => {
            ToolTerminalAction::Input {
                data_base64: request
                    .data_base64
                    .ok_or_else(|| anyhow::anyhow!("missing terminal input"))?,
            }
        }
        "/terminals/resize" if request.data_base64.is_none() => ToolTerminalAction::Resize {
            columns: request
                .columns
                .ok_or_else(|| anyhow::anyhow!("missing terminal columns"))?,
            rows: request
                .rows
                .ok_or_else(|| anyhow::anyhow!("missing terminal rows"))?,
        },
        "/terminals/stop"
            if request.data_base64.is_none()
                && request.columns.is_none()
                && request.rows.is_none() =>
        {
            ToolTerminalAction::Stop
        }
        _ => bail!("invalid terminal operation fields"),
    };
    server_for_session(state, request.session_dir)
        .await?
        .runtime
        .terminal_control(request.id, action)
        .await
}
pub(super) async fn read(
    state: &HttpAppState,
    query: Option<&str>,
) -> Result<ToolTerminalResponse> {
    let [id, cursor, max_bytes, wait_ms] =
        query_params(query, ["id", "cursor", "max_bytes", "wait_ms"])?;
    let id = id
        .ok_or_else(|| anyhow::anyhow!("missing terminal id"))?
        .parse()?;
    let cursor = cursor.map(|value| value.parse()).transpose()?;
    let max_bytes = max_bytes
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(65_536);
    let wait_ms = wait_ms.map(|value| value.parse()).transpose()?.unwrap_or(0);
    server_for_query(state, query)
        .await?
        .runtime
        .terminal_control(
            id,
            ToolTerminalAction::Read {
                cursor,
                max_bytes,
                wait_ms,
            },
        )
        .await
}

//! Public projection of the active client's session. Transcript text is
//! intentionally left to the agent API; metadata updates are incremental.

use proteus_contracts::app_protocol::config::ConfigSummary;
use proteus_contracts::app_protocol::{
    AppExecutionState, AppPendingRequests, AppSessionActivity, AppTranscriptSubagent,
    AppTranscriptTool,
};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Context {
    pub used: u32,
    pub max: u32,
    pub trigger: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct State {
    pub connected: bool,
    pub config: Option<ConfigSummary>,
    pub context: Option<Context>,
    pub execution: AppExecutionState,
    pub pending: Option<AppPendingRequests>,
    pub activity: Option<AppSessionActivity>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub role: &'static str,
    pub tool: Option<AppTranscriptTool>,
    pub subagent: Option<AppTranscriptSubagent>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Update {
    State {
        state: Box<State>,
    },
    Entries {
        start: usize,
        removed: usize,
        entries: Vec<Entry>,
    },
}

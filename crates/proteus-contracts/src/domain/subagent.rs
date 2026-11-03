use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{CallId, SessionId, ThreadId};

/// Immutable display link from one parent facade invocation to its actual
/// durable child session. It contains no copied child history or resume lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentSessionBinding {
    pub source_call_id: CallId,
    pub child_thread_id: ThreadId,
    pub session_id: SessionId,
    pub session_dir: PathBuf,
}

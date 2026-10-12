use std::path::PathBuf;

use gpui_kit::SharedString;
use proteus_client_common::pending::PendingCursor;
use proteus_client_common::sync::SessionCursor;
use proteus_contracts::app_protocol::config::ConfigSummary;
use proteus_contracts::app_protocol::{AppExecutionState, AppPendingRequests, AppRunStatus};
use proteus_contracts::domain::ThreadId;

use super::context_usage::ContextUsage;
use crate::transcript::Transcript;

/// What the client knows about one open session.
pub(in crate::ui) struct Session {
    pub dir: PathBuf,
    pub(super) cursor: SessionCursor,
    pub(super) pending_cursor: PendingCursor,
    pub transcript: Transcript,
    pub execution: AppExecutionState,
    pub pending: Option<AppPendingRequests>,
    pub connected: bool,
    pub link_error: Option<SharedString>,
    pub config: Option<ConfigSummary>,
    pub context: Option<ContextUsage>,
    pub(super) root_thread: Option<ThreadId>,
}

/// What a chat tab says about the run and the connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum ChatState {
    Idle,
    Busy(&'static str),
    Trouble(&'static str),
}

impl Session {
    pub(super) fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            cursor: SessionCursor::default(),
            pending_cursor: PendingCursor::default(),
            transcript: Transcript::default(),
            execution: AppExecutionState::default(),
            pending: None,
            connected: false,
            link_error: None,
            config: None,
            context: None,
            root_thread: None,
        }
    }

    pub fn running(&self) -> bool {
        self.execution.active.is_some()
    }

    pub fn state(&self) -> ChatState {
        if !self.connected {
            return match self.link_error {
                Some(_) => ChatState::Trouble("Переподключение…"),
                None => ChatState::Busy("Подключение…"),
            };
        }
        match self.execution.active.as_ref().map(|run| &run.status) {
            Some(AppRunStatus::CancelRequested) => ChatState::Busy("Остановка…"),
            Some(_) => ChatState::Busy("Работает…"),
            None => ChatState::Idle,
        }
    }
}

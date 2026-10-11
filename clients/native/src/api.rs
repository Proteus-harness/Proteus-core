use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use proteus_contracts::app_protocol::config::ConfigSummary;
use proteus_contracts::app_protocol::config_builder::{ConfigBuilderSnapshot, ConfigHistory};
use proteus_contracts::app_protocol::http::{
    ApprovalRequest, CancelRequest, DeleteQueuedMessageRequest, NewSessionRequest,
    ResumeSessionRequest, SendRequest, SessionOpened, SetConfigBuilderRequest, SetModelRequest,
    SetPermissionModeRequest, SetReasoningEffortRequest, UserInputRequest,
};
use proteus_contracts::app_protocol::{
    AppBootstrap, AppContextMapSnapshot, AppServerEvent, AppSessionSummary, StdioOutput,
};
use proteus_contracts::contracts::{ModelQuotaSnapshot, UserInputResponse};
use proteus_contracts::domain::{MessageId, PermissionMode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Blocking HTTP client for the app server; call it off the UI thread.
#[derive(Clone)]
pub struct Api {
    agent: ureq::Agent,
    origin: Arc<str>,
    token: Arc<str>,
}

impl Api {
    pub fn new(origin: &str, token: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(10)))
            .build()
            .into();
        Self {
            agent,
            origin: origin.into(),
            token: token.into(),
        }
    }

    pub fn bootstrap(&self) -> Result<AppBootstrap> {
        self.get("/bootstrap", None)
    }

    pub fn sessions(&self) -> Result<Vec<AppSessionSummary>> {
        self.get("/sessions", None)
    }

    pub fn resume(&self, session_dir: &Path) -> Result<SessionOpened> {
        let body = ResumeSessionRequest {
            id: Some(request_id("resume")),
            session_dir: session_dir.to_path_buf(),
        };
        self.command("/resume", None, &body)
    }

    pub fn new_session(&self, source: Option<&Path>) -> Result<SessionOpened> {
        let body = NewSessionRequest {
            id: Some(request_id("new-session")),
            source_session_dir: source.map(Path::to_path_buf),
        };
        self.command("/new-session", None, &body)
    }

    pub fn send(&self, request: &SendRequest) -> Result<Value> {
        self.command("/send-async", None, request)
    }

    pub fn cancel(&self, session_dir: &Path, run_id: &str) -> Result<Value> {
        let body = CancelRequest {
            id: Some(request_id("cancel")),
            target_id: run_id.to_owned(),
        };
        self.command("/cancel", Some(session_dir), &body)
    }

    pub fn approve(&self, session_dir: &Path, approval_id: &str, approved: bool) -> Result<Value> {
        let body = ApprovalRequest {
            id: Some(request_id("approval")),
            approval_id: approval_id.to_owned(),
            approved,
            note: None,
            cache: Default::default(),
        };
        self.command("/approval", Some(session_dir), &body)
    }

    pub fn answer_user_input(
        &self,
        session_dir: &Path,
        request_id_value: &str,
        response: UserInputResponse,
    ) -> Result<Value> {
        let body = UserInputRequest {
            id: Some(request_id("user-input")),
            request_id: request_id_value.to_owned(),
            response,
        };
        self.command("/user-input", Some(session_dir), &body)
    }

    pub fn delete_queued(&self, session_dir: &Path, message_id: MessageId) -> Result<Value> {
        let body = DeleteQueuedMessageRequest {
            id: Some(request_id("queue-delete")),
            session_dir: session_dir.to_path_buf(),
            message_id,
        };
        self.command("/queue/delete", None, &body)
    }

    pub fn config(&self, session_dir: &Path) -> Result<ConfigSummary> {
        self.get("/config", Some(session_dir))
    }

    pub fn context(&self, session_dir: &Path) -> Result<AppContextMapSnapshot> {
        self.get("/context", Some(session_dir))
    }

    /// `None` when the active model provider reports no quota.
    pub fn model_quota(&self, session_dir: &Path) -> Result<Option<ModelQuotaSnapshot>> {
        self.get("/model/quota", Some(session_dir))
    }

    pub fn set_mode(&self, session_dir: &Path, mode: PermissionMode) -> Result<ConfigSummary> {
        let body = SetPermissionModeRequest {
            id: Some(request_id("mode")),
            mode,
            session_dir: session_dir.to_path_buf(),
        };
        self.control("/mode", session_dir, &body)
    }

    pub fn set_model(&self, session_dir: &Path, model: &str) -> Result<ConfigSummary> {
        let body = SetModelRequest {
            id: Some(request_id("model")),
            model: model.to_owned(),
            session_dir: session_dir.to_path_buf(),
        };
        self.control("/model", session_dir, &body)
    }

    /// `None` returns to the profile default, `Some("none")` disables reasoning.
    pub fn set_effort(&self, session_dir: &Path, effort: Option<String>) -> Result<ConfigSummary> {
        let body = SetReasoningEffortRequest {
            id: Some(request_id("effort")),
            effort,
            session_dir: session_dir.to_path_buf(),
        };
        self.control("/effort", session_dir, &body)
    }

    /// Settings commands answer with the updated config when the server has it.
    fn control<B: Serialize>(
        &self,
        path: &str,
        session_dir: &Path,
        body: &B,
    ) -> Result<ConfigSummary> {
        let output: Value = self.command(path, None, body)?;
        match output.get("config") {
            Some(config) => Ok(serde_json::from_value(config.clone())?),
            None => self.config(session_dir),
        }
    }

    /// Opens the session event stream; blocks until the server answers.
    pub fn config_builder(&self, session_dir: &Path) -> Result<ConfigBuilderSnapshot> {
        self.get("/config/builder", Some(session_dir))
    }

    pub fn config_history(&self, session_dir: &Path) -> Result<ConfigHistory> {
        self.get("/config/history", Some(session_dir))
    }

    /// Persists profile changes; the server answers with the fresh snapshot
    /// or with a plain-text validation error.
    pub fn save_config_builder(
        &self,
        session_dir: &Path,
        request: &SetConfigBuilderRequest,
    ) -> Result<ConfigBuilderSnapshot> {
        let response = self
            .agent
            .post(format!("{}/config/builder", self.origin))
            .header("Authorization", format!("Bearer {}", self.token))
            .query("session_dir", session_dir.to_string_lossy())
            .send_json(request)?;
        let status = response.status();
        let text = response.into_body().into_with_config().read_to_string()?;
        if !status.is_success() {
            bail!("{}", text.trim());
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub fn events(&self, session_dir: &Path) -> Result<EventStream> {
        let response = self
            .agent
            .get(format!("{}/events", self.origin))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "text/event-stream")
            .query("session_dir", session_dir.to_string_lossy())
            .call()?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .into_body()
                .into_with_config()
                .read_to_string()
                .unwrap_or_default();
            bail!("event stream failed with {status}: {body}");
        }
        Ok(EventStream {
            reader: BufReader::new(Box::new(response.into_body().into_reader())),
        })
    }

    fn get<T: DeserializeOwned>(&self, path: &str, session_dir: Option<&Path>) -> Result<T> {
        let mut request = self
            .agent
            .get(format!("{}{path}", self.origin))
            .header("Authorization", format!("Bearer {}", self.token));
        if let Some(dir) = session_dir {
            request = request.query("session_dir", dir.to_string_lossy());
        }
        let response = request.call()?;
        let status = response.status();
        let body = response.into_body().into_with_config().read_to_string()?;
        if !status.is_success() {
            bail!("GET {path} failed with {status}: {body}");
        }
        Ok(serde_json::from_str(&body)?)
    }

    fn command<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        session_dir: Option<&Path>,
        body: &B,
    ) -> Result<T> {
        let mut request = self
            .agent
            .post(format!("{}{path}", self.origin))
            .header("Authorization", format!("Bearer {}", self.token));
        if let Some(dir) = session_dir {
            request = request.query("session_dir", dir.to_string_lossy());
        }
        let response = request.send_json(body)?;
        let status = response.status();
        let text = response.into_body().into_with_config().read_to_string()?;
        match serde_json::from_str::<StdioOutput>(&text) {
            Ok(output) => proteus_app_common::response::command_output(output)
                .map_err(|error| anyhow!("{path}: {error}")),
            Err(_) => Err(anyhow!("POST {path} failed with {status}: {text}")),
        }
    }
}

fn request_id(kind: &str) -> String {
    format!("{kind}-{}", uuid::Uuid::new_v4().simple())
}

/// Server-sent `output` events of one `/events` connection.
pub struct EventStream {
    reader: BufReader<Box<dyn std::io::Read + Send>>,
}

pub enum StreamItem {
    Event(Box<AppServerEvent>),
    /// Comment line such as the keep-alive heartbeat.
    Heartbeat,
}

impl EventStream {
    /// Returns the next item, or `None` when the server closed the stream.
    pub fn next(&mut self) -> Option<Result<StreamItem>> {
        let mut data = String::new();
        let mut line = String::new();
        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => return None,
                Ok(_) => {}
                Err(error) => return Some(Err(error.into())),
            }
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if data.is_empty() {
                    continue;
                }
                return Some(parse_output(&data).map(StreamItem::Event));
            }
            if line.starts_with(':') {
                if data.is_empty() {
                    return Some(Ok(StreamItem::Heartbeat));
                }
                continue;
            }
            if let Some(value) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value.strip_prefix(' ').unwrap_or(value));
            }
        }
    }
}

fn parse_output(data: &str) -> Result<Box<AppServerEvent>> {
    match serde_json::from_str::<StdioOutput>(data)? {
        StdioOutput::Event { event } => Ok(event),
        _ => Err(anyhow!("unexpected response on the event stream")),
    }
}

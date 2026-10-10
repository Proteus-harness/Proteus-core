use std::path::{Path, PathBuf};

use gpui_kit::*;
use proteus_contracts::app_protocol::config::ConfigSummary;
use proteus_contracts::app_protocol::http::SendRequest;
use proteus_contracts::domain::{MessageId, PermissionMode, RunOptions};

use super::context_usage::ContextUsage;
use super::workspace::Workspace;

impl Workspace {
    pub(super) fn select_session(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.dir == dir)
        {
            return;
        }
        self.call(
            cx,
            move |api| api.resume(&dir),
            |this, opened, cx| this.open_session(opened.session_dir, cx),
        );
    }

    pub(super) fn new_session(&mut self, cx: &mut Context<Self>) {
        let source = self.session_dir();
        self.call(
            cx,
            move |api| {
                let opened = api.new_session(source.as_deref())?;
                Ok((opened, api.sessions()?))
            },
            |this, (opened, sessions), cx| {
                this.sessions = sessions;
                this.open_session(opened.session_dir, cx);
            },
        );
    }

    pub(super) fn refresh_sessions(&mut self, cx: &mut Context<Self>) {
        self.call(
            cx,
            |api| api.sessions(),
            |this, sessions, _| this.sessions = sessions,
        );
    }

    pub(super) fn load_config(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.session_dir() else {
            return;
        };
        let target = dir.clone();
        self.call(
            cx,
            move |api| api.config(&target),
            move |this, config, _| this.apply_config(&dir, config),
        );
    }

    pub(super) fn load_context(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.session_dir() else {
            return;
        };
        let target = dir.clone();
        self.call(
            cx,
            move |api| api.context(&target),
            move |this, context, _| {
                if let Some(session) = this.session.as_mut().filter(|s| s.dir == dir) {
                    session.context = context
                        .latest_usage
                        .as_ref()
                        .and_then(ContextUsage::from_snapshot);
                }
            },
        );
    }

    /// Sends the composer text; the server queues it while a run is active.
    pub(super) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_owned();
        let Some(session) = &self.session else {
            return;
        };
        if text.is_empty() || !session.connected {
            return;
        }
        let request = SendRequest {
            id: Some(format!("run-{}", uuid::Uuid::new_v4().simple())),
            text,
            images: Vec::new(),
            options: RunOptions::default(),
            session_dir: session.dir.clone(),
        };
        self.composer
            .update(cx, |state, cx| state.clean(window, cx));
        self.notice = None;
        self.call(cx, move |api| api.send(&request), |_, _, _| {});
    }

    pub(super) fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        let Some(run) = &session.execution.active else {
            return;
        };
        let (dir, run_id) = (session.dir.clone(), run.run_id.clone());
        self.call(cx, move |api| api.cancel(&dir, &run_id), |_, _, _| {});
    }

    pub(super) fn resolve_approval(
        &mut self,
        approval_id: String,
        approved: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(dir) = self.session_dir() else {
            return;
        };
        self.call(
            cx,
            move |api| api.approve(&dir, &approval_id, approved),
            |_, _, _| {},
        );
    }

    pub(super) fn delete_queued(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let Some(dir) = self.session_dir() else {
            return;
        };
        self.call(
            cx,
            move |api| api.delete_queued(&dir, message_id),
            |_, _, _| {},
        );
    }

    pub(super) fn submit_user_input(&mut self, request_id: String, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        let Some(request) = session.pending.as_ref().and_then(|pending| {
            pending
                .user_inputs
                .iter()
                .find(|request| request.request_id == request_id)
        }) else {
            return;
        };
        let Some(response) = self.user_input.response(request, cx) else {
            self.notice = Some("Ответьте на все вопросы перед отправкой.".into());
            cx.notify();
            return;
        };
        let dir = session.dir.clone();
        self.notice = None;
        self.call(
            cx,
            move |api| api.answer_user_input(&dir, &request_id, response),
            |_, _, _| {},
        );
    }

    pub(super) fn set_mode(&mut self, mode: PermissionMode, cx: &mut Context<Self>) {
        self.update_config(cx, move |api, dir| api.set_mode(dir, mode));
    }

    pub(super) fn set_model(&mut self, model: String, cx: &mut Context<Self>) {
        self.update_config(cx, move |api, dir| api.set_model(dir, &model));
    }

    pub(super) fn set_effort(&mut self, effort: Option<String>, cx: &mut Context<Self>) {
        self.update_config(cx, move |api, dir| api.set_effort(dir, effort));
    }

    fn update_config(
        &mut self,
        cx: &mut Context<Self>,
        job: impl FnOnce(&crate::api::Api, &Path) -> anyhow::Result<ConfigSummary> + Send + 'static,
    ) {
        let Some(dir) = self.session_dir() else {
            return;
        };
        let target = dir.clone();
        self.call(
            cx,
            move |api| job(api, &target),
            move |this, config, _| this.apply_config(&dir, config),
        );
    }

    fn apply_config(&mut self, dir: &Path, config: ConfigSummary) {
        if let Some(session) = self.session.as_mut().filter(|session| session.dir == dir) {
            if let Some(error) = &config.model_catalog_error {
                self.notice = Some(format!("Не удалось загрузить каталог моделей: {error}").into());
            }
            session.config = Some(config);
        }
    }

    fn session_dir(&self) -> Option<PathBuf> {
        self.session.as_ref().map(|session| session.dir.clone())
    }
}

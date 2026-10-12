//! Server calls of a chat: sending, stopping, answers and session config.

use std::path::Path;

use gpui_kit::*;
use proteus_contracts::app_protocol::config::ConfigSummary;
use proteus_contracts::app_protocol::http::SendRequest;
use proteus_contracts::domain::{MessageId, PermissionMode, RunOptions};

use super::context_usage::ContextUsage;
use super::{Chat, ChatEvent};
use crate::api::Api;

impl Chat {
    pub fn load_config(&mut self, cx: &mut Context<Self>) {
        let dir = self.session.dir.clone();
        self.call(
            cx,
            move |api| api.config(&dir),
            |this, config, cx| this.apply_config(config, cx),
        );
    }

    pub(super) fn load_context(&mut self, cx: &mut Context<Self>) {
        let dir = self.session.dir.clone();
        self.call(
            cx,
            move |api| api.context(&dir),
            |this, context, _| {
                this.session.context = context
                    .latest_usage
                    .as_ref()
                    .and_then(ContextUsage::from_snapshot);
            },
        );
    }

    /// Sends the composer text and images; the server queues them while a
    /// run is active. A refused message returns to an empty composer.
    pub(super) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_owned();
        if (text.is_empty() && self.attachments.is_empty()) || !self.session.connected {
            return;
        }
        let draft = std::mem::take(&mut self.attachments);
        let request = SendRequest {
            id: Some(format!("run-{}", uuid::Uuid::new_v4().simple())),
            text: text.clone(),
            images: draft.images(),
            options: RunOptions::default(),
            session_dir: self.session.dir.clone(),
        };
        self.composer
            .update(cx, |state, cx| state.clean(window, cx));
        self.clear_notice(cx);
        self.scroller
            .update(cx, |scroller, cx| scroller.scroll_to_end(cx));
        cx.emit(ChatEvent::Sent);
        let api = self.api.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { api.send(&request) })
                .await;
            let Err(error) = result else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                if this.composer.read(cx).value().is_empty() {
                    this.composer
                        .update(cx, |state, cx| state.set_value(text, window, cx));
                }
                if this.attachments.is_empty() {
                    this.attachments = draft;
                }
                this.show_notice(format!("Не отправлено: {error:#}"), cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(run) = &self.session.execution.active else {
            return;
        };
        let (dir, run_id) = (self.session.dir.clone(), run.run_id.clone());
        self.call(cx, move |api| api.cancel(&dir, &run_id), |_, _, _| {});
    }

    pub(super) fn resolve_approval(
        &mut self,
        approval_id: String,
        approved: bool,
        cx: &mut Context<Self>,
    ) {
        let dir = self.session.dir.clone();
        self.call(
            cx,
            move |api| api.approve(&dir, &approval_id, approved),
            |_, _, _| {},
        );
    }

    pub(super) fn delete_queued(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let dir = self.session.dir.clone();
        self.call(
            cx,
            move |api| api.delete_queued(&dir, message_id),
            |_, _, _| {},
        );
    }

    pub(super) fn submit_user_input(&mut self, request_id: String, cx: &mut Context<Self>) {
        let Some(request) = self.session.pending.as_ref().and_then(|pending| {
            pending
                .user_inputs
                .iter()
                .find(|request| request.request_id == request_id)
        }) else {
            return;
        };
        let Some(response) = self.user_input.response(request, cx) else {
            self.show_notice("Ответьте на все вопросы перед отправкой.", cx);
            return;
        };
        let dir = self.session.dir.clone();
        self.clear_notice(cx);
        self.call(
            cx,
            move |api| api.answer_user_input(&dir, &request_id, response),
            |_, _, _| {},
        );
    }

    pub(in crate::ui) fn set_mode(&mut self, mode: PermissionMode, cx: &mut Context<Self>) {
        self.update_config(cx, move |api, dir| api.set_mode(dir, mode));
    }

    pub(in crate::ui) fn set_model(&mut self, model: String, cx: &mut Context<Self>) {
        self.update_config(cx, move |api, dir| api.set_model(dir, &model));
    }

    pub(in crate::ui) fn set_effort(&mut self, effort: Option<String>, cx: &mut Context<Self>) {
        self.update_config(cx, move |api, dir| api.set_effort(dir, effort));
    }

    fn update_config(
        &mut self,
        cx: &mut Context<Self>,
        job: impl FnOnce(&Api, &Path) -> anyhow::Result<ConfigSummary> + Send + 'static,
    ) {
        let dir = self.session.dir.clone();
        self.call(
            cx,
            move |api| job(api, &dir),
            |this, config, cx| this.apply_config(config, cx),
        );
    }

    fn apply_config(&mut self, config: ConfigSummary, cx: &mut Context<Self>) {
        if let Some(error) = &config.model_catalog_error {
            self.show_notice(format!("Не удалось загрузить каталог моделей: {error}"), cx);
        }
        self.session.config = Some(config);
    }
}

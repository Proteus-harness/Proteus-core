//! Ordered session commands with correlated success/failure replies.

use super::Runtime;
use crate::declarative::protocol::{Outgoing, SessionSetting};
use gpui_kit::*;
use std::path::PathBuf;

pub(super) struct Setting {
    generation: u64,
    id: String,
    session: PathBuf,
    setting: SessionSetting,
}

impl Runtime {
    pub(super) fn set_session(
        &mut self,
        id: String,
        session: PathBuf,
        setting: SessionSetting,
        cx: &mut Context<Self>,
    ) {
        self.settings.push_back(Setting {
            generation: self.generation,
            id,
            session,
            setting,
        });
        self.next_setting(cx);
    }

    fn next_setting(&mut self, cx: &mut Context<Self>) {
        if self.setting_running {
            return;
        }
        let Some(request) = self.settings.pop_front() else {
            return;
        };
        if self.host.session_dir(cx).as_ref() != Some(&request.session)
            || self.host.api(cx).is_none()
        {
            self.send(&Outgoing::CommandResult {
                id: request.id,
                session_dir: request.session,
                config: None,
                error: Some("session command targets an inactive session".into()),
            });
            self.next_setting(cx);
            return;
        }
        let api = self.host.api(cx).unwrap();
        self.setting_running = true;
        cx.spawn(async move |this, cx| {
            let session = request.session.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    match request.setting {
                        SessionSetting::Mode { value } => api.set_mode(&session, value),
                        SessionSetting::Model { value } => api.set_model(&session, &value),
                        SessionSetting::Effort { value } => api.set_effort(&session, value),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.setting_running = false;
                let (config, error) = match result {
                    Ok(config) => (Some(config), None),
                    Err(error) => (None, Some(format!("{error:#}"))),
                };
                if this.host.session_dir(cx).as_ref() == Some(&request.session)
                    && let Some(chat) = this.host.chat(cx)
                {
                    chat.update(cx, |chat, cx| {
                        if let Some(config) = &config {
                            chat.apply_config(config.clone(), cx);
                        } else if let Some(error) = &error {
                            chat.show_notice(error.clone(), cx);
                        }
                    });
                }
                if this.generation == request.generation {
                    this.send(&Outgoing::CommandResult {
                        id: request.id,
                        session_dir: request.session,
                        config: config.map(Box::new),
                        error,
                    });
                }
                this.next_setting(cx);
            });
        })
        .detach();
    }
}

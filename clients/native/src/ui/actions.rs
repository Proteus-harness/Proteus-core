use gpui_kit::*;

use super::settings::{SettingsEvent, SettingsView};
use super::workspace::Workspace;

impl Workspace {
    /// Settings of the active chat's agent profile; needs an open chat.
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        let (Some(api), Some(dir)) = (self.api(), self.active_chat_dir()) else {
            return;
        };
        let dir = dir.to_path_buf();
        let workspace = cx.weak_entity();
        let view = cx.new(|cx| SettingsView::new(workspace, api, dir, window, cx));
        self.settings_subscription = Some(cx.subscribe_in(
            &view,
            window,
            |this, _, event: &SettingsEvent, window, cx| match event {
                SettingsEvent::Saved => this.reload_chat_configs(cx),
                SettingsEvent::Close => {
                    this.close_settings(cx);
                    this.focus_active_chat(window, cx);
                }
            },
        ));
        self.settings = Some(view);
        cx.notify();
    }

    pub(super) fn close_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings.take().is_some() {
            self.settings_subscription = None;
            cx.notify();
        }
    }
}

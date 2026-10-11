//! Chats that finish or start waiting while the window is in the background
//! report through system notifications; «Открыть» shows that chat.

use std::path::{Path, PathBuf};

use gpui_kit::*;
use proteus_contracts::app_protocol::AppSessionActivityStatus;

use super::extensions::OpenDocument;
use super::prefs;
use super::workspace::Workspace;
use crate::notify;
use crate::preferences::Notifications;

impl Workspace {
    pub(super) fn attend(
        &mut self,
        dir: &Path,
        status: AppSessionActivityStatus,
        cx: &mut Context<Self>,
    ) {
        let Some(attention) = self.attention.observe(dir, status) else {
            return;
        };
        let wanted = prefs::current(cx).notifications == Notifications::Background;
        if !wanted || cx.active_window().is_some() {
            return;
        }
        let title = self.session_title(dir, cx);
        let opened = notify::show(&title, attention.body());
        let dir = dir.to_path_buf();
        let handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            if opened.recv().await.is_ok() {
                let _ = cx.update_window(handle, |_, window, cx| {
                    window.activate_window();
                    this.update(cx, |this, cx| show_chat(this, dir, window, cx))
                });
            }
        })
        .detach();
    }
}

fn show_chat(this: &mut Workspace, dir: PathBuf, window: &mut Window, cx: &mut Context<Workspace>) {
    let open = OpenDocument {
        preview: false,
        focus: true,
    };
    this.open_chat(dir, open, window, cx);
}

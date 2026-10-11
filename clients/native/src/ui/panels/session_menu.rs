//! What can be done to a session from the list: rename it, pin it to the
//! top, archive it or delete it on the server. Names, pins and the archive
//! are client preferences; deleting is a server call.

use std::path::{Path, PathBuf};

use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::*;

use crate::sessions::SessionMark;
use crate::tabs;
use crate::ui::prefs;
use crate::ui::workspace::Workspace;

/// The session whose name is being edited in the list.
pub(in crate::ui) struct Renaming {
    pub dir: PathBuf,
    pub input: Entity<InputState>,
    _events: Subscription,
}

/// The menu of one row, both on right click and behind its «⋯» button.
pub(super) fn build(
    menu: PopupMenu,
    dir: &Path,
    mark: SessionMark,
    workspace: &WeakEntity<Workspace>,
) -> PopupMenu {
    let item =
        |label: &'static str,
         run: fn(&mut Workspace, PathBuf, &mut Window, &mut Context<Workspace>)| {
            let (workspace, dir) = (workspace.clone(), dir.to_path_buf());
            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                workspace
                    .update(cx, |this, cx| run(this, dir.clone(), window, cx))
                    .ok();
            })
        };
    let pin = if mark.pinned {
        item("Открепить", |_, dir, _, cx| {
            prefs::update(cx, |p| p.sessions.set_pinned(&dir, false))
        })
    } else {
        item("Закрепить сверху", |_, dir, _, cx| {
            prefs::update(cx, |p| p.sessions.set_pinned(&dir, true))
        })
    };
    let archive = if mark.archived {
        item("Вернуть из архива", |_, dir, _, cx| {
            prefs::update(cx, |p| p.sessions.set_archived(&dir, false))
        })
    } else {
        item("В архив", |_, dir, _, cx| {
            prefs::update(cx, |p| p.sessions.set_archived(&dir, true))
        })
    };
    menu.item(item(
        "Переименовать",
        |this, dir, window, cx| this.start_rename(dir, window, cx),
    ))
    .item(pin)
    .item(archive)
    .separator()
    .item(item("Удалить чат…", |this, dir, window, cx| {
        this.confirm_delete(dir, window, cx)
    }))
}

impl Workspace {
    pub(in crate::ui) fn start_rename(
        &mut self,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self.session_title(&dir, cx);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.finish_rename(true, cx);
            }
        });
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.renaming = Some(Renaming {
            dir,
            input,
            _events: events,
        });
        cx.notify();
    }

    /// Ends editing a name; `save` keeps what was typed.
    pub(super) fn finish_rename(&mut self, save: bool, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        if save {
            let value = renaming.input.read(cx).value().to_string();
            prefs::update(cx, |p| p.sessions.rename(&renaming.dir, &value));
        }
        cx.notify();
    }

    fn confirm_delete(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.session_title(&dir, cx);
        let workspace = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (workspace, dir) = (workspace.clone(), dir.clone());
            alert
                .title("Удалить чат?")
                .description(format!(
                    "«{title}» и вся его история будут удалены с диска. Это нельзя отменить."
                ))
                .ok_text("Удалить")
                .ok_variant(ButtonVariant::Danger)
                .show_cancel(true)
                .cancel_text("Отмена")
                .on_ok(move |_, window, cx| {
                    workspace
                        .update(cx, |this, cx| this.delete_session(dir.clone(), window, cx))
                        .ok();
                    true
                })
        });
    }

    fn delete_session(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab(&tabs::chat(&dir), window, cx);
        let target = dir.clone();
        self.call(
            cx,
            move |api| api.delete_session(&target),
            move |this, _, cx| {
                prefs::update(cx, |p| p.sessions.forget(&dir));
                this.refresh_sessions(cx);
            },
        );
    }
}

//! The «+» at the end of a pane's tab bar: a click opens the new tab
//! menu, a right click the pane's own actions — closing it and locking its
//! size.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::*;

use super::menu;
use crate::tabs::{Pane, PaneId, TabLayout};
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::workspace::Workspace;

impl Workspace {
    pub(super) fn render_pane_controls(
        &self,
        pane: &Pane,
        layout: &TabLayout,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let id = pane.id;
        let key = id.0 as usize;
        // The only pane for chats stays while empty, so closing it does nothing.
        let closable = layout.pane_count() > 1 && !pane.tabs.is_empty();
        let locked = layout.pane_locked(id);
        let workspace = cx.weak_entity();
        let add = kit::button(("pane-add", key))
            .ghost()
            .xsmall()
            .icon(IconName::Plus)
            .hint("Новая вкладка", cx)
            .dropdown_menu_with_anchor(Anchor::TopRight, {
                let workspace = workspace.clone();
                move |popup, _, cx| menu::new_tab_menu(popup, id, &workspace, cx)
            });
        h_flex()
            .id(("pane-controls", key))
            .flex_none()
            .h_full()
            .px_1()
            .child(add)
            .context_menu(move |popup, _, _| pane_menu(popup, id, closable, locked, &workspace))
    }
}

fn pane_menu(
    menu: PopupMenu,
    pane: PaneId,
    closable: bool,
    locked: Option<bool>,
    workspace: &WeakEntity<Workspace>,
) -> PopupMenu {
    let close = workspace.clone();
    let lock = workspace.clone();
    menu.item(
        PopupMenuItem::new("Закрыть область")
            .icon(Icon::new(Lucide::SquareX))
            .disabled(!closable)
            .on_click(move |_, window, cx| {
                close
                    .update(cx, |this, cx| this.close_pane(pane, window, cx))
                    .ok();
            }),
    )
    .item(
        PopupMenuItem::new("Закрепить размер")
            .icon(Icon::new(Lucide::Pin))
            .checked(locked == Some(true))
            .disabled(locked.is_none())
            .on_click(move |_, _, cx| {
                let locked = locked != Some(true);
                lock.update(cx, |this, cx| this.lock_pane(pane, locked, cx))
                    .ok();
            }),
    )
}

//! Menus of the tab strip: the context menu of a tab (pinning, closing,
//! splitting) and the «+» menu with a new session, a terminal and the
//! panels.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{Icon, Side};
use gpui_kit::*;

use crate::tabs::{Edge, PaneId, TabLayout};
use crate::ui::panels;
use crate::ui::workspace::Workspace;

/// What the context menu of one tab offers, read from the layout when the
/// strip renders.
pub(super) struct TabMenu {
    pane: PaneId,
    id: SharedString,
    pinned: bool,
    can_split: bool,
    has_others: bool,
    has_right: bool,
}

impl TabMenu {
    pub(super) fn new(pane: PaneId, id: &str, pinned: bool, layout: &TabLayout) -> Self {
        Self {
            pane,
            id: id.to_string().into(),
            pinned,
            can_split: layout.can_split(id, pane),
            has_others: !layout.others(id).is_empty(),
            has_right: !layout.right_of(id).is_empty(),
        }
    }

    pub(super) fn build(&self, menu: PopupMenu, workspace: &WeakEntity<Workspace>) -> PopupMenu {
        let item = |label: &'static str,
                    enabled: bool,
                    run: fn(&mut Workspace, &str, PaneId, &mut Window, &mut Context<Workspace>)| {
            let (workspace, id, pane) = (workspace.clone(), self.id.clone(), self.pane);
            PopupMenuItem::new(label)
                .disabled(!enabled)
                .on_click(move |_, window, cx| {
                    workspace
                        .update(cx, |this, cx| run(this, &id, pane, window, cx))
                        .ok();
                })
        };
        let pin = if self.pinned {
            "Открепить"
        } else {
            "Закрепить"
        };
        menu.item(item(pin, true, |this, id, _, _, cx| {
            this.toggle_pin(id, cx)
        }))
        .separator()
        .item(item("Закрыть", true, |this, id, _, window, cx| {
            this.close_tab(id, window, cx)
        }))
        .item(item(
            "Закрыть другие",
            self.has_others,
            |this, id, _, window, cx| this.close_other_tabs(id, window, cx),
        ))
        .item(item(
            "Закрыть справа",
            self.has_right,
            |this, id, _, window, cx| this.close_tabs_right(id, window, cx),
        ))
        .separator()
        .item(item(
            "Разделить вправо",
            self.can_split,
            |this, id, pane, window, cx| this.split_tab(id, pane, Edge::Right, window, cx),
        ))
        .item(item(
            "Разделить вниз",
            self.can_split,
            |this, id, pane, window, cx| this.split_tab(id, pane, Edge::Bottom, window, cx),
        ))
    }
}

/// The «+» menu of the strip of `pane`: everything opens in this pane,
/// except an open panel, which comes forward where it is.
pub(super) fn new_tab_menu(
    menu: PopupMenu,
    pane: PaneId,
    workspace: &WeakEntity<Workspace>,
    cx: &App,
) -> PopupMenu {
    let open = {
        let workspace = workspace.clone();
        move |run: fn(&mut Workspace, PaneId, &mut Window, &mut Context<Workspace>)| {
            let workspace = workspace.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                workspace
                    .update(cx, |this, cx| run(this, pane, window, cx))
                    .ok();
            }
        }
    };
    let layout = workspace
        .upgrade()
        .map(|workspace| workspace.read(cx).tab_layout(cx));
    // Items have icons, so the mark of an open panel goes on the right.
    let mut menu = menu
        .check_side(Side::Right)
        .item(
            PopupMenuItem::new("Новая сессия")
                .icon(Icon::new(Lucide::MessageSquarePlus))
                .on_click(open(|this, pane, window, cx| {
                    this.new_session(Some(pane), window, cx)
                })),
        )
        .item(
            PopupMenuItem::new("Терминал")
                .icon(Icon::new(Lucide::SquareTerminal))
                .on_click(open(|this, pane, window, cx| {
                    this.open_terminal(Some(pane), window, cx)
                })),
        )
        .separator();
    for panel in panels::available(cx) {
        let opened = layout
            .as_ref()
            .is_some_and(|layout| layout.contains(&panel.id));
        let workspace = workspace.clone();
        menu = menu.item(
            PopupMenuItem::new(panel.title)
                .icon(panel.icon)
                .checked(opened)
                .on_click(move |_, window, cx| {
                    workspace
                        .update(cx, |this, cx| {
                            this.open_panel(&panel.id, Some(pane), window, cx)
                        })
                        .ok();
                }),
        );
    }
    menu
}

impl Workspace {
    fn toggle_pin(&mut self, id: &str, cx: &mut Context<Self>) {
        let pinned = self.tab_layout(cx).is_pinned(id);
        self.update_tabs(cx, |tabs| tabs.set_pinned(id, !pinned));
    }

    fn close_other_tabs(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.update_tabs(cx, |tabs| tabs.close_others(id));
        self.focus_active(window, cx);
    }

    fn close_tabs_right(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.update_tabs(cx, |tabs| tabs.close_right(id));
        self.focus_active(window, cx);
    }
}

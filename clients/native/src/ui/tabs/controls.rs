//! Buttons at the end of a pane's tab bar: the «+» menu and closing the
//! pane.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::menu;
use crate::tabs::{Pane, TabLayout};
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
        let workspace = cx.weak_entity();
        let add = Button::new(("pane-add", key))
            .ghost()
            .xsmall()
            .icon(IconName::Plus)
            .tooltip("Новая вкладка")
            .dropdown_menu_with_anchor(Anchor::TopRight, move |popup, _, cx| {
                menu::new_tab_menu(popup, id, &workspace, cx)
            });
        h_flex()
            .flex_none()
            .h_full()
            .px_1()
            .gap_1()
            .child(add)
            .when(closable, |controls| {
                controls.child(
                    Button::new(("pane-close", key))
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(Lucide::SquareX))
                        .tooltip("Закрыть область, вкладки перейдут в соседнюю")
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.close_pane(id, window, cx)),
                        ),
                )
            })
    }
}

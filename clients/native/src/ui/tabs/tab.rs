//! One tab of a strip: its look, clicks, drag and context menu. Pinned
//! tabs are compact and have no close button; panels show only an icon.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::menu;
use crate::tabs::{self, PaneId, TabLayout};
use crate::ui::chat::ChatState;
use crate::ui::workspace::Workspace;
use crate::ui::{drag, extensions, panels};

pub(super) const BAR_HEIGHT: f32 = 38.;

/// Payload of a tab being dragged; the preview is a lifted copy of the tab.
#[derive(Clone)]
pub(in crate::ui) struct DraggedTab {
    pub id: String,
    icon: Icon,
    title: SharedString,
    preview: bool,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let foreground = cx.theme().tab_active_foreground;
        drag::lifted(
            h_flex()
                .h(px(BAR_HEIGHT - 4.))
                .max_w(px(240.))
                .px_3()
                .gap_2()
                .text_sm()
                .text_color(foreground)
                .child(self.icon.clone().small())
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .when(self.preview, |title| title.italic())
                        .child(self.title.clone()),
                ),
            cx,
        )
    }
}

struct TabLabel {
    icon: Icon,
    title: SharedString,
    tooltip: Option<SharedString>,
}

/// How a tab sits in its strip.
#[derive(Clone, Copy)]
pub(super) struct TabState {
    pub active: bool,
    pub focused: bool,
    pub preview: bool,
    pub pinned: bool,
}

impl Workspace {
    fn chat_state(&self, id: &str, cx: &App) -> Option<ChatState> {
        Some(self.chats.get(id)?.view.read(cx).session.state())
    }

    fn tab_label(&self, id: &str, cx: &App) -> TabLabel {
        if let Some(dir) = tabs::chat_dir(id) {
            let tooltip = match self.chat_state(id, cx) {
                Some(ChatState::Busy(text) | ChatState::Trouble(text)) => Some(text.into()),
                _ => None,
            };
            return TabLabel {
                icon: Icon::new(Lucide::MessageSquare),
                title: self.session_title(dir).into(),
                tooltip,
            };
        }
        if let Some(panel) = panels::info(id, cx) {
            return TabLabel {
                icon: panel.icon,
                title: panel.title.into(),
                tooltip: Some(panel.title.into()),
            };
        }
        match extensions::document(id, cx) {
            Some((manifest, argument)) => TabLabel {
                icon: Icon::new(IconName::File),
                title: manifest
                    .document
                    .as_ref()
                    .map_or_else(|| argument.clone().into(), |doc| (doc.label)(&argument)),
                tooltip: Some(argument.into()),
            },
            None => TabLabel {
                icon: Icon::new(IconName::File),
                title: id.to_string().into(),
                tooltip: None,
            },
        }
    }

    pub(super) fn render_tab(
        &self,
        pane: PaneId,
        id: &str,
        state: TabState,
        layout: &TabLayout,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let label = self.tab_label(id, cx);
        let tab_group = SharedString::from(format!("tab-{id}"));
        let (hover, primary) = (theme.list_hover, theme.primary);
        let lifted = drag::is_source(&format!("tab:{id}"), cx);
        let icon_only = state.pinned && tabs::is_panel(id);
        let icon = match self.chat_state(id, cx) {
            Some(ChatState::Busy(_)) => Spinner::new().xsmall().into_any_element(),
            Some(ChatState::Trouble(_)) => Icon::new(IconName::TriangleAlert)
                .small()
                .text_color(theme.warning)
                .into_any_element(),
            _ => label.icon.clone().small().into_any_element(),
        };
        let dragged = DraggedTab {
            id: id.to_string(),
            icon: label.icon.clone(),
            title: label.title.clone(),
            preview: state.preview,
        };
        let tooltip = (label.tooltip.clone()).or_else(|| state.pinned.then(|| label.title.clone()));
        let menu = menu::TabMenu::new(pane, id, state.pinned, layout);
        let (drop_id, click_id, close_id, middle_id) = (
            id.to_string(),
            id.to_string(),
            id.to_string(),
            id.to_string(),
        );
        let workspace = cx.weak_entity();
        h_flex()
            .id(ElementId::Name(format!("tab-{id}").into()))
            .group(tab_group.clone())
            .relative()
            .h_full()
            .flex_none()
            .map(|tab| match (icon_only, state.pinned) {
                (true, _) => tab.px_3(),
                (false, true) => tab.max_w(px(140.)).px_3().gap_2(),
                (false, false) => tab.max_w(px(240.)).pl_3().pr(px(4.)).gap_2(),
            })
            .text_sm()
            .cursor_pointer()
            .border_r_1()
            .border_color(theme.border)
            .map(|tab| {
                if state.active {
                    tab.bg(theme.tab_active)
                        .text_color(theme.tab_active_foreground)
                } else {
                    tab.text_color(theme.tab_foreground)
                        .hover(move |style| style.bg(hover))
                }
            })
            .when(lifted, |tab| tab.opacity(drag::SOURCE_OPACITY))
            .child(drag::insertion_mark::<DraggedTab>(tab_group.clone(), cx))
            .child(icon)
            .when(!icon_only, |tab| {
                tab.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .when(state.preview, |title| title.italic())
                        .child(label.title),
                )
            })
            .when(!state.pinned, |tab| {
                tab.child(
                    div()
                        .when(!state.active, |close| {
                            close
                                .invisible()
                                .group_hover(tab_group, |style| style.visible())
                        })
                        .child(
                            Button::new(ElementId::Name(format!("tab-close-{id}").into()))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("Закрыть вкладку")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close_tab(&close_id, window, cx);
                                })),
                        ),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| this.close_tab(&middle_id, window, cx)),
                )
            })
            .when(state.active && state.focused, |tab| {
                tab.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(2.))
                        .bg(primary),
                )
            })
            .when_some(tooltip, |tab, text| {
                tab.tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() > 1 {
                    this.keep_tab(&click_id, cx);
                } else {
                    this.activate_tab(pane, &click_id, window, cx);
                }
            }))
            .on_drag(dragged, {
                let workspace = cx.weak_entity();
                move |dragged, _, _, cx| {
                    drag::start(format!("tab:{}", dragged.id), cx);
                    workspace
                        .update(cx, |this, cx| {
                            this.tab_drag = true;
                            cx.notify();
                        })
                        .ok();
                    cx.new(|_| dragged.clone())
                }
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                this.tab_drag = false;
                this.move_tab(&dragged.id, pane, Some(&drop_id), window, cx);
            }))
            .context_menu(move |popup, _, _| menu.build(popup, &workspace))
    }
}

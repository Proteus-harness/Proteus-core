use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppRunStatus;

use crate::preferences::BadgePlace;
use crate::tabs::{CHAT, TabLayout};
use crate::ui::extensions::{self, drag_preview};
use crate::ui::workspace::Workspace;

const BAR_HEIGHT: f32 = 38.;

/// Payload of a tab being dragged; also its drag preview.
#[derive(Clone)]
pub(in crate::ui) struct DraggedTab {
    pub id: String,
    pub title: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drag_preview(self.title.clone(), cx)
    }
}

/// What the chat tab says about the run and the connection.
enum ChatState {
    Idle,
    Busy(&'static str),
    Trouble(&'static str),
}

impl Workspace {
    fn chat_state(&self) -> ChatState {
        let Some(session) = &self.session else {
            return ChatState::Idle;
        };
        if !session.connected {
            return match session.link_error {
                Some(_) => ChatState::Trouble("Переподключение…"),
                None => ChatState::Busy("Подключение…"),
            };
        }
        match session.execution.active.as_ref().map(|run| &run.status) {
            Some(AppRunStatus::CancelRequested) => ChatState::Busy("Остановка…"),
            Some(_) => ChatState::Busy("Работает…"),
            None => ChatState::Idle,
        }
    }

    fn tab_label(&self, id: &str) -> (Icon, SharedString) {
        if id == CHAT {
            return (
                Icon::new(Lucide::MessageSquare),
                self.session_title().into(),
            );
        }
        match extensions::find(id) {
            Some(manifest) => (
                (manifest.icon)(),
                manifest
                    .page
                    .as_ref()
                    .map_or(manifest.title, |page| page.title)
                    .into(),
            ),
            None => (Icon::new(IconName::File), id.to_string().into()),
        }
    }

    pub(super) fn render_tab_bar(
        &self,
        group: usize,
        layout: &TabLayout,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let drop = theme.drop_target;
        let tabs = &layout.groups[group];
        let focused = layout.focused == group;
        let last = group + 1 == layout.groups.len();
        h_flex()
            .h(px(BAR_HEIGHT))
            .flex_none()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .id(("tab-strip", group))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .children(tabs.tabs.iter().map(|id| {
                        let active = tabs.active.as_deref() == Some(id.as_str());
                        self.render_tab(group, id, active, focused, cx)
                    }))
                    .child(
                        div()
                            .id(("tab-strip-end", group))
                            .flex_1()
                            .min_w(px(24.))
                            .h_full()
                            .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(drop))
                            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                                this.tab_drag = false;
                                this.move_tab(&dragged.id, group, None, window, cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .h_full()
                    .px_1()
                    .gap_0p5()
                    .child(self.add_tab_button(group, layout, cx))
                    .when(layout.is_split() && tabs.active.is_some(), |bar| {
                        bar.child(
                            Button::new(("tab-transfer", group))
                                .ghost()
                                .small()
                                .icon(Icon::new(Lucide::ArrowLeftRight))
                                .tooltip("Перенести вкладку в соседнюю область")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.transfer_tab(group, window, cx)
                                })),
                        )
                    }),
            )
            .when(last, |bar| {
                bar.child(self.render_controls_strip(layout, cx))
            })
    }

    fn render_tab(
        &self,
        group: usize,
        id: &str,
        active: bool,
        focused: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (icon, title) = self.tab_label(id);
        let closable = id != CHAT;
        let hover_group = SharedString::from(format!("tab-{id}"));
        let (hover, drop, primary) = (theme.list_hover, theme.drop_target, theme.primary);
        let icon = match (id == CHAT).then(|| self.chat_state()) {
            Some(ChatState::Busy(_)) => Spinner::new().xsmall().into_any_element(),
            Some(ChatState::Trouble(_)) => Icon::new(IconName::TriangleAlert)
                .small()
                .text_color(theme.warning)
                .into_any_element(),
            _ => icon.small().into_any_element(),
        };
        let tooltip = match (id == CHAT).then(|| self.chat_state()) {
            Some(ChatState::Busy(text) | ChatState::Trouble(text)) => Some(text),
            _ => None,
        };
        let (drag_id, drop_id, click_id, close_id, middle_id) = (
            id.to_string(),
            id.to_string(),
            id.to_string(),
            id.to_string(),
            id.to_string(),
        );
        h_flex()
            .id(ElementId::Name(format!("tab-{id}").into()))
            .group(hover_group.clone())
            .relative()
            .h_full()
            .flex_none()
            .max_w(px(240.))
            .pl_3()
            .pr(if closable { px(4.) } else { px(12.) })
            .gap_2()
            .text_sm()
            .cursor_pointer()
            .border_r_1()
            .border_color(theme.border)
            .map(|tab| {
                if active {
                    tab.bg(theme.tab_active)
                        .text_color(theme.tab_active_foreground)
                } else {
                    tab.text_color(theme.tab_foreground)
                        .hover(move |style| style.bg(hover))
                }
            })
            .child(icon)
            .child(div().min_w_0().truncate().child(title.clone()))
            .when(closable, |tab| {
                tab.child(
                    div()
                        .when(!active, |close| {
                            close
                                .invisible()
                                .group_hover(hover_group, |style| style.visible())
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
            .when(active && focused, |tab| {
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
                tab.tooltip(move |window, cx| Tooltip::new(text).build(window, cx))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.activate_tab(group, &click_id, window, cx)
            }))
            .on_drag(DraggedTab { id: drag_id, title }, {
                let workspace = cx.weak_entity();
                move |dragged, _, _, cx| {
                    workspace
                        .update(cx, |this, cx| {
                            this.tab_drag = true;
                            cx.notify();
                        })
                        .ok();
                    cx.new(|_| dragged.clone())
                }
            })
            .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(drop))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                this.tab_drag = false;
                this.move_tab(&dragged.id, group, Some(&drop_id), window, cx);
            }))
    }

    /// "+" menu with the tabs that can be opened in `group`.
    fn add_tab_button(&self, group: usize, layout: &TabLayout, cx: &Context<Self>) -> AnyElement {
        let mut items: Vec<(&'static str, &'static str, bool)> = Vec::new();
        if layout.group_of(CHAT) != Some(group) {
            items.push((CHAT, "Чат", false));
        }
        for manifest in extensions::visible_pages(cx) {
            if let Some(page) = &manifest.page {
                items.push((
                    manifest.id,
                    page.title,
                    layout.group_of(manifest.id) == Some(group),
                ));
            }
        }
        let workspace = cx.weak_entity();
        Button::new(("tab-add", group))
            .ghost()
            .small()
            .icon(IconName::Plus)
            .tooltip("Открыть вкладку")
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu: PopupMenu, _, _| {
                let mut menu = menu.min_w(px(200.));
                for &(id, title, open) in &items {
                    let workspace = workspace.clone();
                    let icon = if id == CHAT {
                        Icon::new(Lucide::MessageSquare)
                    } else {
                        extensions::find(id)
                            .map_or(Icon::new(IconName::File), |manifest| (manifest.icon)())
                    };
                    menu = menu.item(PopupMenuItem::new(title).icon(icon).checked(open).on_click(
                        move |_, window, cx| {
                            workspace
                                .update(cx, |this, cx| this.open_tab_in(id, group, window, cx))
                                .ok();
                        },
                    ));
                }
                menu
            })
            .into_any_element()
    }

    /// Icons placed in the tab bar, the split toggle and the panel toggle.
    fn render_controls_strip(
        &self,
        layout: &TabLayout,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let split = layout.is_split();
        h_flex()
            .flex_none()
            .h_full()
            .px_2()
            .gap_1()
            .border_l_1()
            .border_color(theme.border)
            .child(self.render_badge_strip(BadgePlace::Header, cx))
            .child(
                Button::new("toggle-split")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::Columns2))
                    .selected(split)
                    .tooltip(if split {
                        "Объединить области"
                    } else {
                        "Разделить область"
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_split(window, cx))),
            )
            .child(
                Button::new("toggle-panel")
                    .ghost()
                    .small()
                    .icon(IconName::PanelRight)
                    .selected(self.panel.open)
                    .tooltip("Боковая панель (Ctrl+Alt+B)")
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_panel(window, cx))),
            )
    }
}

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::tabs::{self, Pane, PaneId, TabLayout};
use crate::ui::chat::ChatState;
use crate::ui::drag;
use crate::ui::extensions;
use crate::ui::workspace::Workspace;

const BAR_HEIGHT: f32 = 38.;

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

    pub(super) fn render_tab_bar(
        &self,
        pane: &Pane,
        layout: &TabLayout,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let id = pane.id;
        let focused = layout.focused == id;
        let end_group = SharedString::from(format!("tab-end-{}", id.0));
        let strip = self
            .tab_strips
            .get(&id)
            .map(|(strip, _)| strip.clone())
            .unwrap_or_default();
        h_flex()
            .h(px(BAR_HEIGHT))
            .flex_none()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .id(("tab-strip", id.0 as usize))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&strip)
                    .children(pane.tabs.iter().map(|tab| {
                        let active = pane.active.as_deref() == Some(tab.as_str());
                        let preview = layout.preview.as_deref() == Some(tab.as_str());
                        self.render_tab(id, tab, active, focused, preview, cx)
                    }))
                    .child(
                        div()
                            .id(("tab-strip-end", id.0 as usize))
                            .group(end_group.clone())
                            .relative()
                            .flex_1()
                            .min_w(px(24.))
                            .h_full()
                            .child(drag::insertion_mark::<DraggedTab>(end_group, cx))
                            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                                this.tab_drag = false;
                                this.move_tab(&dragged.id, id, None, window, cx);
                            })),
                    ),
            )
            .child(self.render_pane_controls(pane, layout, cx))
    }

    fn render_tab(
        &self,
        pane: PaneId,
        id: &str,
        active: bool,
        focused: bool,
        preview: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let label = self.tab_label(id, cx);
        let tab_group = SharedString::from(format!("tab-{id}"));
        let (hover, primary) = (theme.list_hover, theme.primary);
        let lifted = drag::is_source(&format!("tab:{id}"), cx);
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
            preview,
        };
        let (drop_id, click_id, close_id, middle_id) = (
            id.to_string(),
            id.to_string(),
            id.to_string(),
            id.to_string(),
        );
        h_flex()
            .id(ElementId::Name(format!("tab-{id}").into()))
            .group(tab_group.clone())
            .relative()
            .h_full()
            .flex_none()
            .max_w(px(240.))
            .pl_3()
            .pr(px(4.))
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
            .when(lifted, |tab| tab.opacity(drag::SOURCE_OPACITY))
            .child(drag::insertion_mark::<DraggedTab>(tab_group.clone(), cx))
            .child(icon)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .when(preview, |title| title.italic())
                    .child(label.title),
            )
            .child(
                div()
                    .when(!active, |close| {
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
            .when_some(label.tooltip, |tab, text| {
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
    }
}

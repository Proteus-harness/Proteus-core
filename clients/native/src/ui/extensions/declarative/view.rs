//! A surface of an installed extension: panel, document or icon.

use std::collections::HashMap;

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::Value;

use super::render;
use super::runtime::Runtime;
use crate::declarative::protocol::{Outgoing, SurfaceKind};
use crate::declarative::widget::Node;
use crate::ui::kit;

pub(super) struct DeclarativeView {
    runtime: Entity<Runtime>,
    view: String,
    surface: SurfaceKind,
    root: Option<Node>,
    /// Selected tab of every `tabs` node, by tree path.
    pub(super) tabs: HashMap<String, usize>,
    focus: FocusHandle,
    _runtime: Subscription,
}

impl DeclarativeView {
    pub(super) fn new(
        runtime: Entity<Runtime>,
        surface: SurfaceKind,
        argument: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = crate::declarative::protocol::view_id(surface, argument.as_deref());
        let me = cx.weak_entity();
        let (id, attach_runtime) = (view.clone(), runtime.clone());
        // Attach after construction: the runtime may answer synchronously.
        cx.defer(move |cx| {
            attach_runtime.update(cx, |runtime, cx| {
                runtime.attach(id, surface, argument, me, cx)
            });
        });
        let detach = (runtime.clone(), view.clone());
        cx.on_release(move |_, cx| {
            detach.0.update(cx, |runtime, _| runtime.detach(&detach.1));
        })
        .detach();
        Self {
            _runtime: cx.observe(&runtime, |_, _, cx| cx.notify()),
            runtime,
            view,
            surface,
            root: None,
            tabs: HashMap::new(),
            focus: cx.focus_handle(),
        }
    }

    pub(super) fn focus(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(super) fn show(&mut self, root: Node, cx: &mut Context<Self>) {
        self.root = Some(root);
        cx.notify();
    }

    pub(super) fn act(&mut self, action: String, value: Option<Value>, cx: &mut Context<Self>) {
        let message = Outgoing::Action {
            view: self.view.clone(),
            action,
            value,
        };
        self.runtime.read(cx).send(&message);
    }

    /// Face of the compact icon; `None` shows the extension icon.
    pub(super) fn face(&self, cx: &App) -> Option<(String, Hsla)> {
        self.root
            .as_ref()
            .and_then(|root| render::compact(root, cx))
    }

    pub(super) fn error(&self, cx: &App) -> Option<String> {
        self.runtime.read(cx).error.clone()
    }

    fn render_error(&self, error: String, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        v_flex()
            .gap_2()
            .p_3()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.danger.opacity(0.4))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.danger)
                    .child("Расширение не отвечает"),
            )
            .child(
                div()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .child(error),
            )
            .child(
                kit::button("declarative-restart")
                    .small()
                    .outline()
                    .label("Перезапустить")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.runtime.update(cx, |runtime, cx| runtime.restart(cx));
                    })),
            )
    }
}

impl Focusable for DeclarativeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DeclarativeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.error(cx);
        let body = match &self.root {
            Some(root) => {
                let root = root.clone();
                Some(self.node(&root, "root", cx))
            }
            None => None,
        };
        let muted = cx.theme().muted_foreground;
        let title = self.runtime.read(cx).title();
        v_flex()
            .id(ElementId::Name(format!("declarative-{}", self.view).into()))
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .gap_3()
            .when_some(error, |view, error| {
                view.child(self.render_error(error, cx))
            })
            .when_some(body, |view, body| view.child(body))
            .when(
                self.root.is_none() && self.surface != SurfaceKind::Badge,
                |view| {
                    view.child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child(format!("{title}: жду ответа расширения…")),
                    )
                },
            )
    }
}

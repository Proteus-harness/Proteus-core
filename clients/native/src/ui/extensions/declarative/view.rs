//! A surface of an installed extension: panel, document or icon.

use std::collections::HashMap;

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::Value;

use super::render;
use super::runtime::Runtime;
use crate::declarative::presentation::Presentation;
use crate::declarative::protocol::{Outgoing, SurfaceKind};
use crate::declarative::widget::Node;
use crate::ui::kit;

pub(super) struct DeclarativeView {
    runtime: Entity<Runtime>,
    view: String,
    surface: SurfaceKind,
    root: Option<Node>,
    keys: HashMap<String, String>,
    action_sequence: u64,
    pub(super) choice_cursor: Option<usize>,
    pub(super) presentation: Option<Presentation>,
    /// Selected tab of every `tabs` node, by tree path.
    pub(super) tabs: HashMap<String, usize>,
    pub(super) steps: HashMap<String, Entity<super::steps::Stepper>>,
    pub(super) focus: FocusHandle,
    pub(super) fields: super::fields::Fields,
    _runtime: Subscription,
}

impl DeclarativeView {
    pub(super) fn choices(&self) -> Vec<(String, Value, bool)> {
        let mut choices = Vec::new();
        if let Some(root) = &self.root {
            root.visit(&mut |node| {
                if let Node::Choice {
                    action,
                    value,
                    selected,
                    ..
                } = node
                {
                    choices.push((action.clone(), value.clone(), *selected));
                }
            });
        }
        choices
    }

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
            keys: HashMap::new(),
            action_sequence: 0,
            choice_cursor: None,
            presentation: None,
            tabs: HashMap::new(),
            steps: HashMap::new(),
            focus: cx.focus_handle(),
            fields: super::fields::Fields::default(),
        }
    }

    pub(super) fn focus(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(super) fn show(
        &mut self,
        root: Node,
        sequence: u64,
        keys: HashMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        if sequence < self.action_sequence {
            return;
        }
        self.keys = keys;
        self.root = Some(root);
        cx.notify();
    }

    pub(super) fn present(&mut self, presentation: Option<Presentation>, cx: &mut Context<Self>) {
        self.presentation = presentation;
        cx.notify();
    }

    pub(super) fn act(&mut self, action: String, value: Option<Value>, cx: &mut Context<Self>) {
        self.action_sequence += 1;
        let message = Outgoing::Action {
            sequence: self.action_sequence,
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

    pub(super) fn popup_closed(&self, cx: &App) -> u64 {
        self.runtime.read(cx).popup_closed
    }

    pub(super) fn reset(&mut self, cx: &mut Context<Self>) {
        self.root = None;
        self.keys.clear();
        self.presentation = None;
        self.action_sequence = 0;
        self.fields = super::fields::Fields::default();
        self.steps.clear();
        self.tabs.clear();
        cx.notify();
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(root) = self.root.clone() {
            self.prune_fields(&root);
            self.prepare_steps(&root, cx);
            self.prepare_fields(&root, window, cx);
        }
        if self.surface != SurfaceKind::Popup && self.focus.is_focused(window) {
            if let Some(tree) = self.fields.trees.values().next() {
                let focus = tree.read(cx).focus();
                cx.defer_in(window, move |_, window, cx| focus.focus(window, cx));
            }
        }
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
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let chord = crate::shortcuts::normalize(&event.keystroke.unparse()).ok();
                let typing = !event.keystroke.modifiers.modified()
                    && event.keystroke.key != "escape"
                    && this
                        .fields
                        .inputs
                        .values()
                        .any(|input| input.state.read(cx).focus_handle(cx).is_focused(window));
                if !typing
                    && let Some(action) = chord.and_then(|chord| this.keys.get(&chord)).cloned()
                {
                    this.act(action, None, cx);
                    cx.stop_propagation();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if this.surface != SurfaceKind::Popup || event.keystroke.modifiers.modified() {
                    return;
                }
                let choices = this.choices();
                match event.keystroke.key.as_str() {
                    "up" | "down" if !choices.is_empty() => {
                        let at = this.choice_cursor.unwrap_or_else(|| {
                            choices
                                .iter()
                                .position(|(_, _, selected)| *selected)
                                .unwrap_or(0)
                        });
                        this.choice_cursor = Some(if event.keystroke.key == "up" {
                            (at + choices.len() - 1) % choices.len()
                        } else {
                            (at + 1) % choices.len()
                        });
                        cx.notify();
                        cx.stop_propagation();
                    }
                    "enter" if !choices.is_empty() => {
                        let at = this
                            .choice_cursor
                            .unwrap_or_else(|| {
                                choices
                                    .iter()
                                    .position(|(_, _, selected)| *selected)
                                    .unwrap_or(0)
                            })
                            .min(choices.len() - 1);
                        this.act(choices[at].0.clone(), Some(choices[at].1.clone()), cx);
                        cx.stop_propagation();
                    }
                    "left" | "right" if this.steps.len() == 1 => {
                        let stepper = this.steps.values().next().unwrap().clone();
                        let offset = if event.keystroke.key == "left" { -1 } else { 1 };
                        cx.defer(move |cx| {
                            stepper.update(cx, |stepper, cx| stepper.step(offset, cx))
                        });
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .w_full()
            .when(self.surface != SurfaceKind::Popup, |view| view.h_full())
            .overflow_y_scroll()
            .p_3()
            .gap_3()
            .when(matches!(self.root, Some(Node::Fill { .. })), |view| {
                view.p_0().gap_0().overflow_hidden()
            })
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

impl DeclarativeView {
    fn prepare_steps(&mut self, node: &Node, cx: &mut Context<Self>) {
        match node {
            Node::Steps {
                id,
                options,
                selected,
                action,
            } => {
                if let Some(stepper) = self.steps.get(id) {
                    stepper.update(cx, |stepper, cx| {
                        if stepper.options != *options || stepper.selected != *selected {
                            stepper.options = options.clone();
                            stepper.selected = *selected;
                            cx.notify();
                        }
                    });
                } else {
                    let (this, action) = (cx.weak_entity(), action.clone());
                    let stepper = cx.new(|_| {
                        super::steps::Stepper::new(options.clone(), *selected, move |index, cx| {
                            let _ = this.update(cx, |this, cx| {
                                this.act(action.clone(), Some(serde_json::json!(index)), cx)
                            });
                        })
                    });
                    self.steps.insert(id.clone(), stepper);
                }
            }
            Node::Stack { children, .. }
            | Node::Card { children, .. }
            | Node::Fill { children } => {
                for child in children {
                    self.prepare_steps(child, cx);
                }
            }
            Node::Tabs { tabs } => {
                for tab in tabs {
                    for child in &tab.children {
                        self.prepare_steps(child, cx);
                    }
                }
            }
            _ => {}
        }
    }
}

impl DeclarativeView {
    pub(super) fn focus_control(&self, id: &str, window: &mut Window, cx: &mut App) {
        let focus = self
            .fields
            .inputs
            .get(id)
            .map(|input| input.state.read(cx).focus_handle(cx))
            .or_else(|| self.fields.trees.get(id).map(|tree| tree.read(cx).focus()))
            .or_else(|| {
                self.fields
                    .codes
                    .get(id)
                    .map(|editor| editor.read(cx).focus_handle(cx))
            })
            .or_else(|| {
                self.fields
                    .terminals
                    .get(id)
                    .map(|terminal| terminal.read(cx).focus_handle(cx))
            });
        if let Some(focus) = focus {
            focus.focus(window, cx);
        }
    }
}

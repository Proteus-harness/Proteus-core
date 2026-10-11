use gpui_kit::base::SelectableText;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::text::TextView;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppTranscriptSubagent;

use super::Chat;
use super::tool_row::{render_tool, tool_line};
use crate::transcript::{Entry, Role};
use crate::ui::format;
use crate::ui::motion::{self, chevron, fade_in_if, reveal};

pub(in crate::ui) const CONTENT_WIDTH: f32 = 840.;

impl Chat {
    pub(super) fn toggle_expanded(&mut self, key: u64, cx: &mut Context<Self>) {
        let open = !self.expanded.remove(&key);
        if open {
            self.expanded.insert(key);
        }
        motion::toggled(reveal_key(cx.entity_id(), key), open, cx);
        let row = self
            .session
            .transcript
            .entries()
            .iter()
            .position(|entry| entry.key == key);
        if let Some(row) = row {
            self.scroller.update(cx, |scroller, cx| {
                scroller.remeasure_items(row..row + 1, cx)
            });
        }
        cx.notify();
    }

    pub(super) fn render_row(
        &mut self,
        ix: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entries = self.session.transcript.entries();
        let Some(entry) = entries.get(ix) else {
            return div().into_any_element();
        };
        let expanded = self.expanded.contains(&entry.key);
        let fresh = self.fresh_rows.is_fresh(entry.key);
        let key = entry.key;
        let last = ix + 1 == entries.len();
        let spacing = match (&entry.role, &entry.tool) {
            (Role::User, _) => Spacing {
                top: 4.,
                bottom: 0.,
            },
            (Role::Assistant, _) => Spacing {
                top: 2.,
                bottom: 0.,
            },
            (Role::System, Some(_)) => Spacing {
                top: 1.,
                bottom: 1.,
            },
            (Role::System, None) => Spacing {
                top: 2.,
                bottom: 2.,
            },
        };
        let body = match (&entry.role, &entry.tool, &entry.subagent) {
            (Role::User, _, _) => render_user(entry, cx),
            (Role::Assistant, _, _) => render_assistant(entry),
            (Role::System, Some(tool), _) => render_tool(entry.key, tool, expanded, cx),
            (Role::System, None, Some(agent)) => render_subagent(entry.key, agent, expanded, cx),
            (Role::System, None, None) => render_note(entry, cx),
        };
        h_flex()
            .w_full()
            .justify_center()
            .px_6()
            .pt(px(spacing.top * 4.))
            .pb(px(spacing.bottom * 4.))
            .when(ix == 0, |row| row.pt_6())
            .when(last, |row| row.pb_6())
            .child(fade_in_if(
                div().w_full().max_w(px(CONTENT_WIDTH)).child(body),
                row_id("enter", key),
                fresh,
            ))
            .into_any_element()
    }
}

/// Vertical padding of a row in 4px units: tool calls sit tightly together,
/// a user turn opens with extra space.
struct Spacing {
    top: f32,
    bottom: f32,
}

fn row_id(name: &'static str, key: u64) -> ElementId {
    ElementId::NamedInteger(name.into(), key)
}

/// Motion key of a row's expandable details; row keys repeat across chats.
pub(super) fn reveal_key(chat: EntityId, key: u64) -> SharedString {
    format!("thread-row-{chat}-{key}").into()
}

fn copy_button(name: &'static str, key: u64, text: SharedString) -> Clipboard {
    Clipboard::new(row_id(name, key))
        .value(text)
        .tooltip("Копировать")
        .xsmall()
}

fn render_user(entry: &Entry, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let text = SharedString::from(&entry.text);
    let images = (entry.images > 0).then(|| {
        div()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(format!("Изображений: {}", entry.images))
    });
    v_flex()
        .group(ROW_GROUP)
        .w_full()
        .items_end()
        .gap_0p5()
        .child(
            v_flex()
                .max_w(relative(0.85))
                .px_3()
                .py_2()
                .gap_1()
                .rounded(theme.radius * 2.)
                .bg(theme.secondary)
                .text_color(theme.secondary_foreground)
                .child(SelectableText::new(row_id("user", entry.key), text.clone()))
                .children(images),
        )
        .child(row_actions().child(copy_button("copy-user", entry.key, text)))
        .into_any_element()
}

fn render_assistant(entry: &Entry) -> AnyElement {
    let text = SharedString::from(&entry.text);
    let actions = (!entry.streaming && !entry.text.trim().is_empty())
        .then(|| row_actions().child(copy_button("copy-assistant", entry.key, text.clone())));
    v_flex()
        .group(ROW_GROUP)
        .w_full()
        .child(
            TextView::markdown(row_id("assistant", entry.key), text)
                .selectable(true)
                .stream_fade(true)
                .code_block_actions(|block, _, _| {
                    let code = block.code();
                    Clipboard::new(row_id("copy-code", hash_code(&code)))
                        .value(code)
                        .tooltip("Копировать код")
                        .xsmall()
                }),
        )
        .children(actions)
        .into_any_element()
}

const ROW_GROUP: &str = "thread-row";

/// Row actions stay laid out so revealing them on hover does not shift rows.
fn row_actions() -> Div {
    h_flex()
        .h_5()
        .gap_1()
        .invisible()
        .group_hover(ROW_GROUP, |style| style.visible())
}

fn hash_code(code: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hasher);
    hasher.finish()
}

fn render_note(entry: &Entry, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(format::clip(&entry.text, 200))
        .into_any_element()
}

fn render_subagent(
    key: u64,
    agent: &AppTranscriptSubagent,
    expanded: bool,
    cx: &mut Context<Chat>,
) -> AnyElement {
    let theme = cx.theme();
    let mut meta = format!("{} · инструментов: {}", agent.status, agent.tools.len());
    if let Some(iterations) = agent.iterations {
        meta.push_str(&format!(" · шагов: {iterations}"));
    }
    let app: &App = cx;
    let tools = reveal(reveal_key(cx.entity_id(), key), expanded, app, || {
        v_flex()
            .pl_5()
            .gap_1()
            .children(agent.tools.iter().map(|tool| tool_line(tool, app)))
            .into_any_element()
    });
    v_flex()
        .px_3()
        .py_2()
        .gap_2()
        .rounded(theme.radius * 2.)
        .border_1()
        .border_color(theme.border)
        .child(
            h_flex()
                .id(ElementId::NamedInteger("subagent".into(), key))
                .gap_2()
                .text_sm()
                .cursor_pointer()
                .child(Icon::new(IconName::Bot).small())
                .child(div().flex_none().font_semibold().child(agent.role.clone()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.muted_foreground)
                        .child(agent.description.clone().unwrap_or_default()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(meta),
                )
                .child(chevron(
                    row_id("subagent-chevron", key),
                    expanded,
                    theme.muted_foreground,
                ))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(key, cx))),
        )
        .children(tools)
        .into_any_element()
}

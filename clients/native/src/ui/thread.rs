use gpui_kit::base::SelectableText;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::{AppTranscriptSubagent, AppTranscriptTool};

use super::format;
use super::workspace::Workspace;
use crate::transcript::{Entry, Role};

pub(super) const CONTENT_WIDTH: f32 = 840.;

const RESULT_LINES: usize = 40;
const RESULT_CHARS: usize = 8_000;

impl Workspace {
    pub(super) fn render_row(
        &mut self,
        ix: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self
            .session
            .as_ref()
            .and_then(|session| session.transcript.entries().get(ix))
        else {
            return div().into_any_element();
        };
        let expanded = self.expanded.contains(&entry.key);
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
            .py_1p5()
            .child(div().w_full().max_w(px(CONTENT_WIDTH)).child(body))
            .into_any_element()
    }
}

fn row_id(name: &'static str, key: u64) -> ElementId {
    ElementId::NamedInteger(name.into(), key)
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
        .pt_2()
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
        .h_6()
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

fn status_icon(status: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    match status {
        "done" => Icon::new(IconName::Check)
            .xsmall()
            .text_color(theme.success)
            .into_any_element(),
        "failed" | "denied" => Icon::new(IconName::CircleX)
            .xsmall()
            .text_color(theme.danger)
            .into_any_element(),
        "waiting_approval" => Icon::new(IconName::CircleAlert)
            .xsmall()
            .text_color(theme.warning)
            .into_any_element(),
        _ => Spinner::new().xsmall().into_any_element(),
    }
}

fn tool_line(tool: &AppTranscriptTool, cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .gap_2()
        .text_sm()
        .min_w_0()
        .child(status_icon(&tool.status, cx))
        .child(
            div()
                .flex_none()
                .font_family(theme.mono_font_family.clone())
                .child(tool.name.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme.muted_foreground)
                .child(format::tool_summary(tool)),
        )
}

fn code_block(id: ElementId, text: String, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .w_full()
        .px_3()
        .py_2()
        .rounded(theme.radius)
        .bg(theme.muted)
        .font_family(theme.mono_font_family.clone())
        .text_xs()
        .child(SelectableText::new(id, text))
}

fn render_tool(
    key: u64,
    tool: &AppTranscriptTool,
    expanded: bool,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let args = expanded.then(|| {
        let args = tool.effective_args.as_ref().unwrap_or(&tool.args);
        let pretty = serde_json::to_string_pretty(args).unwrap_or_default();
        code_block(
            row_id("tool-args", key),
            format::clip_block(&pretty, RESULT_LINES, RESULT_CHARS),
            cx,
        )
    });
    let result = expanded
        .then(|| tool.result.as_deref())
        .flatten()
        .filter(|result| !result.trim().is_empty())
        .map(|result| {
            code_block(
                row_id("tool-result", key),
                format::clip_block(result, RESULT_LINES, RESULT_CHARS),
                cx,
            )
        });
    let chevron = if expanded {
        IconName::ChevronDown
    } else {
        IconName::ChevronRight
    };
    let muted = cx.theme().muted_foreground;
    v_flex()
        .gap_1()
        .child(
            tool_line(tool, cx)
                .id(ElementId::NamedInteger("tool".into(), key))
                .cursor_pointer()
                .child(Icon::new(chevron).xsmall().text_color(muted))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(key, cx))),
        )
        .children(args)
        .children(result)
        .into_any_element()
}

fn render_subagent(
    key: u64,
    agent: &AppTranscriptSubagent,
    expanded: bool,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme();
    let mut meta = format!("{} · инструментов: {}", agent.status, agent.tools.len());
    if let Some(iterations) = agent.iterations {
        meta.push_str(&format!(" · шагов: {iterations}"));
    }
    let tools = expanded.then(|| {
        v_flex()
            .pl_5()
            .gap_1()
            .children(agent.tools.iter().map(|tool| tool_line(tool, cx)))
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
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(key, cx))),
        )
        .children(tools)
        .into_any_element()
}

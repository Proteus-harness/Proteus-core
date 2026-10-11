use gpui_kit::base::SelectableText;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppTranscriptTool;
use serde_json::Value;

use super::format;
use super::motion::{chevron, reveal};
use super::workspace::Workspace;
use crate::tool_display::{self, DiffLine, PatchFile, PlanStep, StepStatus};

const RESULT_LINES: usize = 40;
const RESULT_CHARS: usize = 8_000;
const DIFF_LINES: usize = 400;
const DIFF_CHARS: usize = 40_000;
/// Subjects longer than this are repeated in full when the row is expanded.
const SHORT_SUBJECT: usize = 100;

fn row_id(name: &'static str, key: u64) -> ElementId {
    ElementId::NamedInteger(name.into(), key)
}

pub(super) fn tool_args(tool: &AppTranscriptTool) -> &Value {
    tool.effective_args.as_ref().unwrap_or(&tool.args)
}

pub(super) fn status_icon(status: &str, cx: &App) -> AnyElement {
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

/// One-line headline: status, action label, subject and patch stats.
pub(super) fn tool_line(tool: &AppTranscriptTool, cx: &App) -> Div {
    let theme = cx.theme();
    let headline = tool_display::headline(&tool.name, tool_args(tool));
    let subject = headline.subject.map(|subject| {
        div()
            .min_w_0()
            .truncate()
            .font_family(theme.mono_font_family.clone())
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(subject)
    });
    h_flex()
        .gap_2()
        .text_sm()
        .min_w_0()
        .child(status_icon(&tool.status, cx))
        .child(div().flex_none().child(headline.label))
        .children(subject)
        .children(
            headline
                .stats
                .filter(|stats| *stats != (0, 0))
                .map(|stats| diff_stats(stats, cx)),
        )
}

fn diff_stats((added, removed): (usize, usize), cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .flex_none()
        .gap_1()
        .text_xs()
        .child(div().text_color(theme.success).child(format!("+{added}")))
        .child(div().text_color(theme.danger).child(format!("−{removed}")))
}

pub(super) fn render_tool(
    key: u64,
    tool: &AppTranscriptTool,
    expanded: bool,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let app: &App = cx;
    let details = reveal(super::thread::reveal_key(key), expanded, app, || {
        tool_details(key, tool, app).into_any_element()
    });
    v_flex()
        .gap_1()
        .child(
            tool_line(tool, cx)
                .id(row_id("tool", key))
                .cursor_pointer()
                .child(chevron(row_id("tool-chevron", key), expanded, muted))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(key, cx))),
        )
        .children(details)
        .into_any_element()
}

fn tool_details(key: u64, tool: &AppTranscriptTool, cx: &App) -> Div {
    let args = tool_args(tool);
    let details = v_flex().pl_5().gap_2();
    let patch = (tool.name == tool_display::APPLY_PATCH)
        .then(|| tool_display::patch_text(args))
        .flatten()
        .map(tool_display::patch_files)
        .filter(|files| !files.is_empty());
    let plan = (tool.name == tool_display::UPDATE_PLAN)
        .then(|| tool_display::plan_steps(args))
        .filter(|steps| !steps.is_empty());
    let details = match (patch, plan) {
        (Some(files), _) => details.children(
            files
                .iter()
                .enumerate()
                .map(|(ix, file)| patch_file(key, ix, file, cx)),
        ),
        (None, Some(steps)) => details.child(plan_list(&steps, cx)),
        (None, None) => details.children(argument_details(key, tool, cx)),
    };
    let failed = matches!(tool.status.as_str(), "failed" | "denied");
    let show_result = failed
        || !matches!(
            tool.name.as_str(),
            tool_display::APPLY_PATCH | tool_display::UPDATE_PLAN
        );
    let result = tool
        .result
        .as_deref()
        .filter(|result| show_result && !result.trim().is_empty())
        .map(|result| {
            code_block(
                row_id("tool-result", key),
                format::clip_block(result, RESULT_LINES, RESULT_CHARS),
                cx,
            )
            .when(failed, |block| {
                block
                    .border_1()
                    .border_color(cx.theme().danger.opacity(0.5))
            })
        });
    details.children(result)
}

/// Tool name, the full subject when the headline had to cut it, and the
/// remaining arguments as `key: value` lines.
fn argument_details(key: u64, tool: &AppTranscriptTool, cx: &App) -> Vec<AnyElement> {
    let theme = cx.theme();
    let args = tool_args(tool);
    let headline = tool_display::headline(&tool.name, args);
    let subject_key = headline.subject_key.as_deref();
    let full_subject = subject_key
        .and_then(|key| args.get(key))
        .and_then(Value::as_str)
        .filter(|text| text.contains('\n') || text.chars().count() > SHORT_SUBJECT)
        .map(|text| {
            code_block(
                row_id("tool-subject", key),
                format::clip_block(text, RESULT_LINES, RESULT_CHARS),
                cx,
            )
            .into_any_element()
        });
    let mono = theme.mono_font_family.clone();
    let rows = tool_display::other_args(args, subject_key)
        .into_iter()
        .map(|(name, value)| {
            h_flex()
                .gap_2()
                .min_w_0()
                .text_xs()
                .font_family(mono.clone())
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.muted_foreground)
                        .child(format!("{name}:")),
                )
                .child(div().min_w_0().truncate().child(value))
                .into_any_element()
        });
    let name = div()
        .text_xs()
        .font_family(mono.clone())
        .text_color(theme.muted_foreground)
        .child(tool.name.clone())
        .into_any_element();
    std::iter::once(name)
        .chain(full_subject)
        .chain(rows)
        .collect()
}

fn patch_file(key: u64, ix: usize, file: &PatchFile, cx: &App) -> Div {
    let theme = cx.theme();
    let body = SharedString::from(file.body.clone());
    let header = h_flex()
        .gap_2()
        .min_w_0()
        .text_xs()
        .child(
            div()
                .flex_none()
                .text_color(theme.muted_foreground)
                .child(file.operation.label()),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family(theme.mono_font_family.clone())
                .child(file.path.clone()),
        )
        .when(file.additions + file.deletions > 0, |header| {
            header.child(diff_stats((file.additions, file.deletions), cx))
        })
        .child(div().flex_1())
        .when(!file.body.is_empty(), |header| {
            header.child(
                Clipboard::new(ElementId::NamedInteger(
                    format!("copy-diff-{ix}").into(),
                    key,
                ))
                .value(body.clone())
                .tooltip("Копировать")
                .xsmall(),
            )
        });
    v_flex()
        .gap_1()
        .child(header)
        .when(!file.body.is_empty(), |file| {
            file.child(diff_block(&body, cx))
        })
}

/// Patch hunks with added and removed lines tinted; long patches are cut.
fn diff_block(body: &str, cx: &App) -> Div {
    let theme = cx.theme();
    let mut text = String::new();
    let mut highlights = Vec::new();
    let total = body.lines().count();
    for (shown, line) in body.lines().enumerate() {
        if shown == DIFF_LINES || text.len() + line.len() > DIFF_CHARS {
            text.push_str(&format!("… ещё строк: {}", total - shown));
            break;
        }
        let color = match DiffLine::of(line) {
            DiffLine::Added => Some(theme.success),
            DiffLine::Removed => Some(theme.danger),
            DiffLine::Hunk => Some(theme.info),
            DiffLine::Context => None,
        };
        if let Some(color) = color {
            highlights.push((
                text.len()..text.len() + line.len(),
                HighlightStyle {
                    color: Some(color),
                    background_color: Some(color.opacity(0.08)),
                    ..Default::default()
                },
            ));
        }
        text.push_str(line);
        text.push('\n');
    }
    text.truncate(text.trim_end().len());
    highlights.retain(|(range, _)| range.end <= text.len());
    div()
        .w_full()
        .px_3()
        .py_2()
        .rounded(theme.radius)
        .bg(theme.muted)
        .font_family(theme.mono_font_family.clone())
        .text_xs()
        .child(StyledText::new(text).with_highlights(highlights))
}

pub(super) fn plan_list(steps: &[PlanStep], cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .gap_1()
        .text_sm()
        .children(steps.iter().map(|step| {
            let (icon, color) = match step.status {
                StepStatus::Completed => (IconName::CircleCheck, theme.success),
                StepStatus::InProgress => (IconName::LoaderCircle, theme.warning),
                StepStatus::Pending => (IconName::Minus, theme.muted_foreground),
            };
            h_flex()
                .gap_2()
                .items_start()
                .child(Icon::new(icon).xsmall().text_color(color).mt_0p5())
                .child(
                    div()
                        .min_w_0()
                        .when(step.status == StepStatus::Completed, |text| {
                            text.text_color(theme.muted_foreground)
                        })
                        .child(step.step.clone()),
                )
        }))
}

pub(super) fn code_block(id: ElementId, text: String, cx: &App) -> Div {
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

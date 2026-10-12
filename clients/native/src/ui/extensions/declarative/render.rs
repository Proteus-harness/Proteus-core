//! Native rendering of the widget tree an extension sends.

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::Value;

use super::view::DeclarativeView;
use crate::declarative::widget::{Direction, Gap, Item, Node, TextSize, Tone, Variant};
use crate::ui::kit;
use crate::ui::prefs;

pub(super) fn tone_color(tone: Tone, cx: &App) -> Hsla {
    let theme = cx.theme();
    match tone {
        Tone::Default => theme.foreground,
        Tone::Muted => theme.muted_foreground,
        Tone::Accent => theme.primary,
        Tone::Success => theme.success,
        Tone::Warning => theme.warning,
        Tone::Danger => theme.danger,
    }
}

impl DeclarativeView {
    /// `path` is the position in the tree; it keys element ids and the
    /// selected tab, so they survive a new tree of the same shape.
    pub(super) fn node(&self, node: &Node, path: &str, cx: &mut Context<Self>) -> AnyElement {
        match node {
            Node::Stack {
                direction,
                gap,
                children,
            } => {
                let base = match direction {
                    Direction::Column => v_flex(),
                    Direction::Row => h_flex().flex_wrap().items_start(),
                };
                let base = match gap {
                    Gap::None => base,
                    Gap::Small => base.gap_1p5(),
                    Gap::Medium => base.gap_3(),
                    Gap::Large => base.gap_5(),
                };
                base.min_w_0()
                    .children(self.children(children, path, cx))
                    .into_any_element()
            }
            Node::Card { title, children } => {
                let theme = cx.theme();
                v_flex()
                    .min_w_0()
                    .gap_2()
                    .p_3()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.secondary)
                    .when_some(title.clone(), |card, title| {
                        card.child(div().text_sm().font_semibold().child(title))
                    })
                    .children(self.children(children, path, cx))
                    .into_any_element()
            }
            Node::Text {
                text,
                tone,
                size,
                mono,
            } => {
                let text = div()
                    .min_w_0()
                    .text_color(tone_color(*tone, cx))
                    .child(text.clone());
                let text = match size {
                    TextSize::Small => text.text_xs(),
                    TextSize::Normal => text.text_sm(),
                    TextSize::Large => text.text_lg().font_semibold(),
                };
                text.when(*mono, |text| {
                    text.font_family(cx.theme().mono_font_family.clone())
                })
                .into_any_element()
            }
            Node::Metric {
                label,
                value,
                detail,
                tone,
            } => {
                let muted = cx.theme().muted_foreground;
                v_flex()
                    .min_w(prefs::scaled(110.))
                    .gap_0p5()
                    .child(div().text_xs().text_color(muted).child(label.clone()))
                    .child(
                        div()
                            .text_lg()
                            .font_semibold()
                            .text_color(tone_color(*tone, cx))
                            .child(value.clone()),
                    )
                    .when_some(detail.clone(), |metric, detail| {
                        metric.child(div().text_xs().text_color(muted).child(detail))
                    })
                    .into_any_element()
            }
            Node::Progress { value, max, label } => {
                let theme = cx.theme();
                let fraction = (value / max).clamp(0., 1.) as f32;
                v_flex()
                    .gap_1()
                    .when_some(label.clone(), |progress, label| {
                        progress.child(
                            h_flex()
                                .justify_between()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(label)
                                .child(format!("{:.0}%", fraction * 100.)),
                        )
                    })
                    .child(
                        div().h_1p5().w_full().rounded_full().bg(theme.muted).child(
                            div()
                                .h_full()
                                .rounded_full()
                                .bg(theme.primary)
                                .w(relative(fraction)),
                        ),
                    )
                    .into_any_element()
            }
            Node::Table { columns, rows } => table(columns, rows, cx).into_any_element(),
            Node::BarChart { bars, unit } => {
                let peak = bars.iter().map(|bar| bar.value).fold(0., f64::max);
                let theme = cx.theme();
                let unit = unit.clone().unwrap_or_default();
                v_flex()
                    .gap_1()
                    .children(bars.iter().map(|bar| {
                        let fraction = if peak > 0. {
                            (bar.value / peak) as f32
                        } else {
                            0.
                        };
                        let color = match bar.tone {
                            Tone::Default => theme.primary,
                            tone => tone_color(tone, cx),
                        };
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .child(
                                div()
                                    .w(prefs::scaled(120.))
                                    .flex_none()
                                    .truncate()
                                    .text_color(theme.muted_foreground)
                                    .child(bar.label.clone()),
                            )
                            .child(
                                div().flex_1().h_2().child(
                                    div()
                                        .h_full()
                                        .rounded(theme.radius / 2.)
                                        .bg(color)
                                        .w(relative(fraction)),
                                ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .min_w(prefs::scaled(56.))
                                    .text_right()
                                    .child(format!("{}{unit}", number(bar.value))),
                            )
                    }))
                    .into_any_element()
            }
            Node::Tabs { tabs } => {
                let selected = self
                    .tabs
                    .get(path)
                    .copied()
                    .unwrap_or(0)
                    .min(tabs.len().saturating_sub(1));
                let theme = cx.theme();
                let (active, muted, border) =
                    (theme.foreground, theme.muted_foreground, theme.border);
                let strip = h_flex().gap_1().border_b_1().border_color(border).children(
                    tabs.iter().enumerate().map(|(index, tab)| {
                        let key = path.to_string();
                        div()
                            .id(ElementId::Name(format!("decl-tab-{path}-{index}").into()))
                            .px_2()
                            .py_1()
                            .text_sm()
                            .cursor_pointer()
                            .text_color(if index == selected { active } else { muted })
                            .when(index == selected, |tab| {
                                tab.border_b_2().border_color(active)
                            })
                            .child(tab.label.clone())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.tabs.insert(key.clone(), index);
                                cx.notify();
                            }))
                    }),
                );
                let body = tabs
                    .get(selected)
                    .map(|tab| self.children(&tab.children, &format!("{path}.{selected}"), cx))
                    .unwrap_or_default();
                v_flex()
                    .gap_2()
                    .child(strip)
                    .child(v_flex().gap_1p5().children(body))
                    .into_any_element()
            }
            Node::List { items } => v_flex()
                .children(
                    items
                        .iter()
                        .enumerate()
                        .map(|(index, item)| self.item(item, &format!("{path}.{index}"), cx)),
                )
                .into_any_element(),
            Node::Button {
                label,
                action,
                value,
                variant,
            } => {
                let send = self.sender(action.clone(), value.clone());
                let button = kit::button(ElementId::Name(format!("decl-button-{path}").into()))
                    .small()
                    .label(label.clone())
                    .on_click(cx.listener(move |this, _, _, cx| send(this, cx)));
                match variant {
                    Variant::Primary => button.primary(),
                    Variant::Default => button.outline(),
                    Variant::Ghost => button.ghost(),
                    Variant::Danger => button.danger(),
                }
                .into_any_element()
            }
            Node::Badge { label, tone } => {
                let color = tone_color(*tone, cx);
                div()
                    .px_1p5()
                    .rounded(cx.theme().radius / 2.)
                    .border_1()
                    .border_color(color.opacity(0.4))
                    .text_xs()
                    .text_color(color)
                    .child(label.clone())
                    .into_any_element()
            }
            Node::Divider {} => div()
                .h(px(1.))
                .w_full()
                .bg(cx.theme().border)
                .into_any_element(),
        }
    }

    fn children(&self, nodes: &[Node], path: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        nodes
            .iter()
            .enumerate()
            .map(|(index, node)| self.node(node, &format!("{path}.{index}"), cx))
            .collect()
    }

    fn item(&self, item: &Item, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let hover = theme.list_hover;
        let row = v_flex()
            .id(ElementId::Name(format!("decl-item-{path}").into()))
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .child(div().text_sm().child(item.title.clone()))
            .when_some(item.detail.clone(), |row, detail| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(detail),
                )
            });
        match &item.action {
            Some(action) => {
                let send = self.sender(action.clone(), item.value.clone());
                row.cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| send(this, cx)))
                    .into_any_element()
            }
            None => row.into_any_element(),
        }
    }

    fn sender(
        &self,
        action: String,
        value: Option<Value>,
    ) -> impl Fn(&mut Self, &mut Context<Self>) + 'static {
        move |this, cx| this.act(action.clone(), value.clone(), cx)
    }
}

fn table(columns: &[String], rows: &[Vec<String>], cx: &App) -> Div {
    let theme = cx.theme();
    let line = |cells: &[String], head: bool| {
        h_flex()
            .gap_3()
            .py_1()
            .border_b_1()
            .border_color(theme.border)
            .text_xs()
            .when(head, |row| {
                row.text_color(theme.muted_foreground).font_semibold()
            })
            .children(
                cells
                    .iter()
                    .map(|cell| div().flex_1().min_w_0().truncate().child(cell.clone())),
            )
    };
    v_flex()
        .min_w_0()
        .child(line(columns, true))
        .children(rows.iter().map(|row| line(row, false)))
}

/// Integers without a fraction, others with up to two digits.
fn number(value: f64) -> String {
    if value.fract() == 0. && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

/// The face of a compact icon: short text and its color taken from a badge,
/// text, metric or progress at the top of the tree.
pub(super) fn compact(node: &Node, cx: &App) -> Option<(String, Hsla)> {
    match node {
        Node::Badge { label, tone }
        | Node::Text {
            text: label, tone, ..
        } => Some((label.clone(), tone_color(*tone, cx))),
        Node::Metric { value, tone, .. } => Some((value.clone(), tone_color(*tone, cx))),
        Node::Progress { value, max, .. } => Some((
            format!("{:.0}%", (value / max).clamp(0., 1.) * 100.),
            cx.theme().foreground,
        )),
        Node::Stack { children, .. } => children.first().and_then(|child| compact(child, cx)),
        _ => None,
    }
}

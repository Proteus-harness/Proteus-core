//! Shared look of side panel widgets: a header that collapses the widget and
//! drags it to another place, and the body below.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::ui::motion::{chevron, rise_in};
use crate::ui::prefs;

const HEADER_GROUP: &str = "widget-header";

/// Payload of a widget being dragged in the panel; also its drag preview.
#[derive(Clone)]
pub(in crate::ui) struct DraggedWidget {
    pub id: &'static str,
    pub title: SharedString,
}

impl Render for DraggedWidget {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .px_3()
            .py_1p5()
            .gap_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_md()
            .text_xs()
            .font_semibold()
            .text_color(theme.popover_foreground)
            .child(
                Icon::new(Lucide::GripVertical)
                    .xsmall()
                    .text_color(theme.muted_foreground),
            )
            .child(self.title.clone())
    }
}

pub(in crate::ui) fn is_open(id: &str, cx: &App) -> bool {
    !prefs::current(cx).extensions.is_collapsed(id)
}

/// A widget section. `body` is `None` while the widget is collapsed, so
/// widgets can skip building it.
pub(in crate::ui) fn widget_frame(
    id: &'static str,
    title: SharedString,
    summary: Option<SharedString>,
    body: Option<AnyElement>,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (muted, hover) = (theme.muted_foreground, theme.list_hover);
    let open = body.is_some();
    let dragged = DraggedWidget { id, title };
    v_flex()
        .flex_none()
        .border_b_1()
        .border_color(theme.border)
        .child(
            h_flex()
                .id((id, 0usize))
                .group(HEADER_GROUP)
                .px_4()
                .py_2()
                .gap_2()
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(chevron((id, 1usize), open, muted))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .font_semibold()
                        .text_color(muted)
                        .child(dragged.title.clone()),
                )
                .children(summary.map(|summary| div().text_xs().text_color(muted).child(summary)))
                .child(
                    div()
                        .invisible()
                        .group_hover(HEADER_GROUP, |style| style.visible())
                        .child(Icon::new(Lucide::GripVertical).xsmall().text_color(muted)),
                )
                .on_click(move |_, _, cx| {
                    prefs::update(cx, |prefs| prefs.extensions.toggle_collapsed(id))
                })
                .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone())),
        )
        .when_some(body, |this, body| {
            this.child(rise_in(div().px_4().pb_3().child(body), (id, 2usize)))
        })
}

/// Label and value on one line.
pub(in crate::ui) fn row(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    cx: &App,
) -> Div {
    h_flex()
        .w_full()
        .gap_3()
        .justify_between()
        .text_sm()
        .child(
            div()
                .flex_none()
                .text_color(cx.theme().muted_foreground)
                .child(label.into()),
        )
        .child(div().min_w_0().truncate().child(value.into()))
}

pub(in crate::ui) fn muted(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

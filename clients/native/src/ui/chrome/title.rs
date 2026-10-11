//! The window's own title bar: it moves and zooms the window and, when the
//! window is client-decorated, draws quiet minimize/maximize/close icons
//! that only change colour on hover.

use std::cell::Cell;

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::ui::prefs;

pub(super) fn title_bar<L: IntoElement, R: IntoElement>(
    left: L,
    right: R,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<L, R> {
    let theme = cx.theme();
    let (background, border) = (theme.title_bar, theme.title_bar_border);
    let decorated = matches!(window.window_decorations(), Decorations::Client { .. });
    let moving = window.use_state(cx, |_, _| Cell::new(false));
    let (down, up, out, drag) = (moving.clone(), moving.clone(), moving.clone(), moving);
    h_flex()
        .id("title-bar")
        .flex_shrink_0()
        .h(prefs::scaled(super::BAR_HEIGHT))
        .pl_1()
        .items_center()
        .bg(background)
        .border_b_1()
        .border_color(border)
        .window_control_area(WindowControlArea::Drag)
        .on_double_click(|_, window, _| window.zoom_window())
        // Controls inside the bar (buttons, draggable icons) prevent default
        // on mouse down; only a press on the bar itself moves the window.
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            down.read(cx).set(!window.default_prevented())
        })
        .on_mouse_up(MouseButton::Left, move |_, _, cx| up.read(cx).set(false))
        .on_mouse_down_out(move |_, _, cx| out.read(cx).set(false))
        .on_mouse_move(move |_, window, cx| {
            if drag.read(cx).replace(false) {
                window.start_window_move();
            }
        })
        .when(decorated, |bar| {
            bar.on_mouse_down(MouseButton::Right, |event, window, _| {
                window.show_window_menu(event.position)
            })
        })
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .items_center()
                .justify_between()
                .pr_1()
                .child(left)
                .child(right),
        )
        .when(decorated, |bar| bar.child(controls(window, cx)))
}

#[derive(Clone, Copy)]
enum Control {
    Minimize,
    Maximize,
    Restore,
    Close,
}

fn controls(window: &Window, cx: &App) -> impl IntoElement + use<> {
    let supported = window.window_controls();
    let zoom = if window.is_maximized() {
        Control::Restore
    } else {
        Control::Maximize
    };
    h_flex()
        .h_full()
        .flex_shrink_0()
        .when(supported.minimize, |row| {
            row.child(control(Control::Minimize, cx))
        })
        .when(supported.maximize, |row| row.child(control(zoom, cx)))
        .child(control(Control::Close, cx))
}

fn control(kind: Control, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (id, icon, hover) = match kind {
        Control::Minimize => ("minimize", IconName::WindowMinimize, theme.foreground),
        Control::Maximize => ("maximize", IconName::WindowMaximize, theme.foreground),
        Control::Restore => ("restore", IconName::WindowRestore, theme.foreground),
        Control::Close => ("close", IconName::WindowClose, theme.danger),
    };
    div()
        .id(id)
        .flex()
        .h_full()
        .w(prefs::scaled(super::BAR_HEIGHT))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .text_color(theme.muted_foreground)
        .hover(move |style| style.text_color(hover))
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            match kind {
                Control::Minimize => window.minimize_window(),
                Control::Maximize | Control::Restore => window.zoom_window(),
                Control::Close => window.remove_window(),
            }
        })
        .child(Icon::new(icon).small())
}

//! Terminal tab: draws snapshots of the headless terminal and feeds it keys.
//!
//! Keys are taken by a keystroke interceptor while the tab is focused, so
//! shell shortcuts (Ctrl+N, Tab, Ctrl+C) reach the shell before the app's
//! bindings. Layout, pane and settings shortcuts stay with the app.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::Size;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::super::Host;
use crate::terminal::{Base, Grid, Key, Run, Signal, Terminal, encode};

const PADDING: f32 = 8.;
const LINE_HEIGHT: f32 = 1.3;

enum State {
    Waiting,
    Running(Terminal),
    Failed(String),
    Exited,
}

pub(super) struct TerminalView {
    focus: FocusHandle,
    cwd: Option<PathBuf>,
    state: State,
    area: Rc<Cell<Option<Size<Pixels>>>>,
    scroll_rest: f32,
    _tasks: Vec<Task<()>>,
    _intercept: Subscription,
}

impl TerminalView {
    pub(super) fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        // The host is busy building this view; read the workspace later.
        cx.defer_in(window, move |this, _, cx| {
            this.cwd = Some(
                host.workspace_path(cx)
                    .or_else(std::env::home_dir)
                    .unwrap_or_else(|| PathBuf::from("/")),
            );
            cx.notify();
        });
        let weak = cx.weak_entity();
        let keys = focus.clone();
        let intercept = cx.intercept_keystrokes(move |event, window, cx| {
            if !keys.is_focused(window) {
                return;
            }
            let handled = weak
                .update(cx, |this, cx| this.key(&event.keystroke, cx))
                .unwrap_or(false);
            if handled {
                cx.stop_propagation();
            }
        });
        Self {
            focus,
            cwd: None,
            state: State::Waiting,
            area: Rc::default(),
            scroll_rest: 0.,
            _tasks: Vec::new(),
            _intercept: intercept,
        }
    }

    pub(super) fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }

    fn start(&mut self, grid: Grid, cx: &mut Context<Self>) {
        let Some(cwd) = self.cwd.clone() else {
            return;
        };
        let spawned = match Terminal::spawn(&cwd, grid) {
            Ok(spawned) => spawned,
            Err(error) => {
                self.state = State::Failed(format!("{error:#}"));
                return;
            }
        };
        let redraw = spawned.redraw;
        let signals = spawned.signals;
        self._tasks = vec![
            cx.spawn(async move |this, cx| {
                while redraw.recv().await.is_ok() {
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }),
            cx.spawn(async move |this, cx| {
                while let Ok(signal) = signals.recv().await {
                    let exited = signal == Signal::Exited;
                    let alive = this.update(cx, |this, cx| {
                        if exited {
                            this.state = State::Exited;
                            cx.notify();
                        }
                    });
                    if exited || alive.is_err() {
                        break;
                    }
                }
            }),
        ];
        self.state = State::Running(spawned.terminal);
    }

    fn restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state = State::Waiting;
        self._tasks.clear();
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Feeds one keystroke to the shell; `false` leaves it to the app.
    fn key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let State::Running(terminal) = &self.state else {
            return false;
        };
        let modifiers = &keystroke.modifiers;
        if modifiers.platform || reserved(keystroke) {
            return false;
        }
        if modifiers.control && modifiers.shift && keystroke.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                terminal.paste(&text);
            }
            return true;
        }
        if modifiers.control && modifiers.shift {
            return false;
        }
        let key = Key {
            key: &keystroke.key,
            text: keystroke.key_char.as_deref(),
            ctrl: modifiers.control,
            alt: modifiers.alt,
            shift: modifiers.shift,
        };
        match encode(key, terminal.app_cursor()) {
            Some(bytes) => {
                terminal.write(bytes);
                true
            }
            None => false,
        }
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, line: Pixels) {
        let State::Running(terminal) = &self.state else {
            return;
        };
        self.scroll_rest += event.delta.pixel_delta(line).y.as_f32() / line.as_f32();
        let lines = self.scroll_rest.trunc();
        if lines != 0. {
            self.scroll_rest -= lines;
            terminal.scroll(lines as i32);
        }
    }

    /// Fits the grid to the measured area; starts the shell once it is known.
    fn fit(&mut self, cell: Size<Pixels>, cx: &mut Context<Self>) {
        let Some(area) = self.area.get() else {
            return;
        };
        let columns = ((area.width.as_f32() - 2. * PADDING) / cell.width.as_f32()).floor();
        let rows = ((area.height.as_f32() - 2. * PADDING) / cell.height.as_f32()).floor();
        let grid = Grid::new(columns.max(0.) as usize, rows.max(0.) as usize);
        match &mut self.state {
            State::Running(terminal) => terminal.resize(grid),
            State::Waiting => self.start(grid, cx),
            State::Failed(_) | State::Exited => {}
        }
    }
}

/// App shortcuts that stay global even inside the terminal.
fn reserved(keystroke: &Keystroke) -> bool {
    let modifiers = &keystroke.modifiers;
    let key = keystroke.key.as_str();
    if modifiers.control && modifiers.alt {
        return matches!(key, "left" | "right" | "up" | "down");
    }
    modifiers.control && matches!(key, "tab" | "," | "\\" | "|" | "=" | "+" | "-" | "0" | "`")
}

fn rgb_bytes(color: Hsla) -> [u8; 3] {
    let color = color.to_rgb();
    [color.r, color.g, color.b].map(|value| (value * 255.).round() as u8)
}

fn color([r, g, b]: [u8; 3]) -> Hsla {
    rgb(u32::from_be_bytes([0, r, g, b])).into()
}

fn cell_size(font: &Font, size: Pixels, window: &Window) -> Size<Pixels> {
    let system = window.text_system();
    let width = system
        .advance(system.resolve_font(font), size, 'm')
        .map(|advance| advance.width)
        .unwrap_or(size * 0.6);
    Size {
        width,
        height: (size * LINE_HEIGHT).round(),
    }
}

fn render_run(run: &Run, cell: Size<Pixels>) -> Div {
    let style = &run.style;
    div()
        .absolute()
        .top_0()
        .left(cell.width * run.column as f32)
        .h(cell.height)
        .when_some(style.background, |this, background| {
            this.w(cell.width * run.width as f32).bg(color(background))
        })
        .whitespace_nowrap()
        .text_color(color(style.foreground))
        .when(style.bold, |this| this.font_weight(FontWeight::BOLD))
        .when(style.italic, |this| this.italic())
        .when(style.underline, |this| this.underline())
        .when(style.strike, |this| this.line_through())
        .child(run.text.clone())
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let family = theme.mono_font_family.clone();
        let size = theme.mono_font_size;
        let background = theme.background;
        let foreground = theme.foreground;
        let muted = theme.muted_foreground;
        let cell = cell_size(&font(family.clone()), size, window);
        self.fit(cell, cx);

        let area = self.area.clone();
        let probe = canvas(
            move |bounds, _, _| area.set(Some(bounds.size)),
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let base = div()
            .id("terminal")
            .size_full()
            .relative()
            .overflow_hidden()
            .track_focus(&self.focus)
            .bg(background)
            .font_family(family)
            .text_size(size)
            .line_height(cell.height)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.focus.focus(window, cx)),
            )
            .on_scroll_wheel(cx.listener(move |this, event, _, cx| {
                this.scroll(event, cell.height);
                cx.notify();
            }))
            .child(probe);

        let notice = |text: String, cx: &mut Context<Self>| {
            v_flex()
                .absolute()
                .bottom(px(PADDING))
                .left(px(PADDING))
                .gap_2()
                .text_color(muted)
                .child(text)
                .child(
                    Button::new("terminal-restart")
                        .small()
                        .primary()
                        .label("Перезапустить")
                        .on_click(cx.listener(|this, _, window, cx| this.restart(window, cx))),
                )
        };
        match &self.state {
            State::Waiting => base,
            State::Failed(error) => {
                base.child(notice(format!("Терминал не запустился: {error}"), cx))
            }
            State::Exited => base.child(notice("Процесс оболочки завершился.".into(), cx)),
            State::Running(terminal) => {
                let screen = terminal.snapshot(Base {
                    foreground: rgb_bytes(foreground),
                    background: rgb_bytes(background),
                });
                let focused = self.focus.is_focused(window);
                let rows = screen.rows.iter().enumerate().map(|(index, runs)| {
                    div()
                        .absolute()
                        .top(px(PADDING) + cell.height * index as f32)
                        .left(px(PADDING))
                        .right(px(PADDING))
                        .h(cell.height)
                        .children(runs.iter().map(|run| render_run(run, cell)))
                });
                let cursor = screen.cursor.map(|(row, column)| {
                    div()
                        .absolute()
                        .top(px(PADDING) + cell.height * row as f32)
                        .left(px(PADDING) + cell.width * column as f32)
                        .w(cell.width)
                        .h(cell.height)
                        .map(|this| {
                            if focused {
                                this.bg(foreground.opacity(0.55))
                            } else {
                                this.border_1().border_color(foreground.opacity(0.6))
                            }
                        })
                });
                base.children(rows).children(cursor)
            }
        }
    }
}

//! Shared pieces of the analytics pages: page header, cards, metrics,
//! badges, select menus and the fetch state with visible-only polling.

use crate::ui::kit;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::ui::prefs;

/// One request kind's state: last good data survives a failed refresh.
pub(super) struct Fetch<T> {
    pub data: Option<T>,
    pub error: Option<String>,
    pub loading: bool,
    generation: u64,
    at: Option<Instant>,
    timer: Option<Task<()>>,
}

impl<T> Default for Fetch<T> {
    fn default() -> Self {
        Self {
            data: None,
            error: None,
            loading: false,
            generation: 0,
            at: None,
            timer: None,
        }
    }
}

impl<T: Send + 'static> Fetch<T> {
    /// Forgets data of another session.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn stale(&self, every: Option<Duration>) -> bool {
        !self.loading
            && match self.at {
                None => true,
                Some(at) => every.is_some_and(|every| at.elapsed() >= every),
            }
    }

    /// Runs `job` off the UI thread; `slot` picks this fetch out of the view.
    pub fn start<V: 'static>(
        &mut self,
        job: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
        slot: fn(&mut V) -> &mut Fetch<T>,
        cx: &mut Context<V>,
    ) {
        self.loading = true;
        self.generation += 1;
        let generation = self.generation;
        let task = cx.background_spawn(async move { job() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                let fetch = slot(view);
                if fetch.generation != generation {
                    return;
                }
                fetch.loading = false;
                fetch.at = Some(Instant::now());
                match result {
                    Ok(data) => {
                        fetch.data = Some(data);
                        fetch.error = None;
                    }
                    Err(error) => fetch.error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Wakes the view after `every`; a hidden page is not rendered, so it
    /// neither refetches nor schedules the next wake-up.
    pub fn poll<V: 'static>(
        &mut self,
        every: Duration,
        slot: fn(&mut V) -> &mut Fetch<T>,
        cx: &mut Context<V>,
    ) {
        if self.timer.is_some() {
            return;
        }
        self.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(every).await;
            this.update(cx, |view, cx| {
                slot(view).timer = None;
                cx.notify();
            })
            .ok();
        }));
    }
}

pub(super) fn header(
    title: &'static str,
    subtitle: impl Into<SharedString>,
    actions: impl IntoIterator<Item = AnyElement>,
    cx: &App,
) -> Div {
    h_flex()
        .w_full()
        .gap_3()
        .items_start()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_xl().font_semibold().child(title))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(subtitle.into()),
                ),
        )
        .child(h_flex().flex_none().gap_2().children(actions))
}

pub(super) fn card(title: impl Into<SharedString>, cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .gap_2()
        .p_3()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(theme.muted_foreground)
                .child(title.into()),
        )
}

pub(super) fn metric(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    detail: Option<String>,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    v_flex()
        .min_w(prefs::scaled(120.))
        .flex_1()
        .gap_0p5()
        .p_3()
        .rounded(theme.radius)
        .bg(theme.secondary)
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label.into()),
        )
        .child(div().text_lg().font_semibold().child(value.into()))
        .children(detail.map(|detail| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(detail)
        }))
}

pub(super) fn kv(key: impl Into<SharedString>, value: impl Into<SharedString>, cx: &App) -> Div {
    h_flex()
        .gap_3()
        .text_sm()
        .child(
            div()
                .w(prefs::scaled(150.))
                .flex_none()
                .text_color(cx.theme().muted_foreground)
                .child(key.into()),
        )
        .child(div().flex_1().min_w_0().child(value.into()))
}

#[derive(Clone, Copy)]
pub(super) enum Tone {
    Muted,
    Accent,
    Success,
    Warning,
    Danger,
}

pub(super) fn tone_color(tone: Tone, cx: &App) -> Hsla {
    let theme = cx.theme();
    match tone {
        Tone::Muted => theme.muted_foreground,
        Tone::Accent => theme.primary,
        Tone::Success => theme.success,
        Tone::Warning => theme.warning,
        Tone::Danger => theme.danger,
    }
}

pub(super) fn badge(text: impl Into<SharedString>, tone: Tone, cx: &App) -> Div {
    let color = tone_color(tone, cx);
    div()
        .flex_none()
        .px_1p5()
        .py(px(1.))
        .rounded(px(4.))
        .text_xs()
        .text_color(color)
        .bg(color.opacity(0.12))
        .child(text.into())
}

/// Scrollable preformatted block for JSON and long texts.
pub(super) fn code(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    div()
        .id(id)
        .max_h(prefs::scaled(360.))
        .overflow_y_scroll()
        .p_2()
        .rounded(theme.radius)
        .bg(theme.secondary)
        .font_family(theme.mono_font_family.clone())
        .text_xs()
        .whitespace_normal()
        .child(text.into())
        .into_any_element()
}

pub(super) fn note(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

pub(super) fn error_line(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().danger)
        .child(text.into())
}

pub(super) struct Choice<T> {
    pub label: SharedString,
    pub value: T,
    pub checked: bool,
}

pub(super) fn select<T: Clone + 'static>(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    choices: Vec<Choice<T>>,
    pick: impl Fn(T, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let pick = Rc::new(pick);
    let choices = Rc::new(choices);
    kit::button(id)
        .label(label)
        .outline()
        .small()
        .dropdown_caret(true)
        .dropdown_menu(move |menu: PopupMenu, _, _| {
            let menu = menu.min_w(px(220.)).max_h(px(420.)).scrollable(true);
            choices.iter().fold(menu, |menu, choice| {
                let (pick, value) = (pick.clone(), choice.value.clone());
                menu.item(
                    PopupMenuItem::new(choice.label.clone())
                        .checked(choice.checked)
                        .on_click(move |_, window, cx| pick(value.clone(), window, cx)),
                )
            })
        })
        .into_any_element()
}

pub(super) fn refresh_button<V: 'static>(
    id: &'static str,
    loading: bool,
    cx: &Context<V>,
    refresh: fn(&mut V, &mut Context<V>),
) -> AnyElement {
    kit::button(id)
        .icon(IconName::Redo2)
        .label("Обновить")
        .ghost()
        .small()
        .loading(loading)
        .on_click(cx.listener(move |view, _, _, cx| {
            refresh(view, cx);
            cx.notify();
        }))
        .into_any_element()
}

/// Segmented tabs; `pick` receives the index.
pub(super) fn segments<V: 'static>(
    id: &'static str,
    labels: &[&'static str],
    selected: usize,
    cx: &Context<V>,
    pick: fn(&mut V, usize, &mut Context<V>),
) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .gap_1()
        .p_0p5()
        .rounded(theme.radius)
        .bg(theme.secondary)
        .children(labels.iter().enumerate().map(|(index, label)| {
            let active = index == selected;
            div()
                .id((id, index))
                .px_3()
                .py_1()
                .rounded(theme.radius)
                .text_sm()
                .cursor_pointer()
                .when(active, |tab| {
                    tab.bg(theme.background).font_semibold().shadow_xs()
                })
                .when(!active, |tab| {
                    tab.text_color(theme.muted_foreground)
                        .hover(|tab| tab.text_color(theme.foreground))
                })
                .child(*label)
                .on_click(cx.listener(move |view, _, _, cx| pick(view, index, cx)))
        }))
        .into_any_element()
}

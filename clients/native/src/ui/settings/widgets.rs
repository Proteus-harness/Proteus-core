//! Building blocks of settings pages: sections, cards, rows, chips, radio
//! cards and select menus.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::ui::fade::faded;
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::prefs;
use crate::ui::tip::Tip as _;

/// A row title narrower than this gives the line to the control instead;
/// down to it a long title fades out at its end.
const TITLE_MIN_WIDTH: f32 = 160.;
/// Text inputs on a row's line narrow down to this before moving below.
const INPUT_MIN_WIDTH: f32 = 140.;
const INPUT_MAX_WIDTH: f32 = 220.;
/// Inputs for numbers and other short values.
pub(super) const SHORT_INPUT_WIDTH: f32 = 112.;
/// Controls under the title stop growing here, so lists stay readable.
const BELOW_MAX_WIDTH: f32 = 560.;
/// Tooltips with descriptions wrap at this width.
const ABOUT_WIDTH: f32 = 320.;
/// Tooltip text is cut here: tool descriptions written for the model run to
/// pages.
const ABOUT_CHARS: usize = 600;

pub(super) fn hashed_id(name: &'static str, key: &impl Hash) -> ElementId {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    ElementId::NamedInteger(name.into(), hasher.finish())
}

/// Marks unsaved edits in navigation, headings and rows.
pub(super) fn changed_dot(cx: &App) -> impl IntoElement + use<> {
    div()
        .size(px(6.))
        .flex_none()
        .rounded_full()
        .bg(cx.theme().warning)
}

/// Titled part of a page: heading with an optional hint and action, then
/// the body.
pub(super) fn section(
    title: impl Into<SharedString>,
    hint: Option<SharedString>,
    action: Option<AnyElement>,
    body: impl IntoElement,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    v_flex()
        .gap_2()
        .child(
            h_flex()
                .gap_3()
                .items_end()
                .px_1()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(div().text_sm().font_semibold().child(title.into()))
                        .children(hint.map(|hint| {
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(hint)
                        })),
                )
                .children(action),
        )
        .child(body)
}

/// Rounded card with hairline dividers between rows.
pub(super) fn card(rows: impl IntoIterator<Item = AnyElement>, cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .bg(theme.group_box)
        .overflow_hidden()
        .children(rows.into_iter().enumerate().map(|(ix, row)| {
            div()
                .when(ix > 0, |cell| cell.border_t_1().border_color(theme.border))
                .child(row)
        }))
}

/// Muted explanatory text inside a card or under a heading.
pub(super) fn note(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .px_4()
        .py_3()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// Title on the left, the control on the right; wide controls go under the
/// title, and a narrow window moves the control to its own line instead of
/// squeezing the title into a column of letters. The description lives in
/// a tooltip on the title, so rows stay one line high.
pub(super) fn row(
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    control: impl IntoElement,
    vertical: bool,
    cx: &App,
) -> Div {
    let title: SharedString = title.into();
    let id = hashed_id("row-title", &title);
    let title = described(id, title, description, None, cx);
    field_row(
        title,
        control,
        if vertical { Fit::Below } else { Fit::Fixed },
    )
}

/// How a row places its control.
#[derive(Clone, Copy)]
pub(super) enum Fit {
    /// At its own width on the right.
    Fixed,
    /// On the right, sharing the line with the title: it narrows down to a
    /// minimum before taking a line of its own.
    Shrink,
    /// Under the title at full width.
    Below,
}

/// `row` with a title line built by the caller.
pub(super) fn field_row(title: impl IntoElement, control: impl IntoElement, fit: Fit) -> Div {
    let title = h_flex()
        .min_w_0()
        .gap_x_2()
        .items_center()
        .text_sm()
        .child(title);
    if let Fit::Below = fit {
        return v_flex().px_4().py_2p5().gap_2().child(title).child(
            div()
                .w_full()
                .max_w(prefs::scaled(BELOW_MAX_WIDTH))
                .child(control),
        );
    }
    // Both share the line alike, so a wide line gives the control its full
    // width and the rest to the title.
    let cell = match fit {
        Fit::Shrink => div()
            .flex_1()
            .min_w(prefs::scaled(INPUT_MIN_WIDTH))
            .max_w(prefs::scaled(INPUT_MAX_WIDTH)),
        _ => div().flex_none(),
    };
    h_flex()
        .flex_wrap()
        .px_4()
        .py_2p5()
        .gap_x_6()
        .gap_y_2()
        .items_center()
        .child(title.flex_1().min_w(prefs::scaled(TITLE_MIN_WIDTH)))
        .child(cell.ml_auto().child(control))
}

/// The title on one line, fading out where it runs out of room, with an
/// info mark; hovering either shows the whole title, `about` and the config
/// `key`. Without both it is the bare title.
pub(super) fn described(
    id: ElementId,
    title: SharedString,
    about: Option<SharedString>,
    key: Option<SharedString>,
    cx: &App,
) -> AnyElement {
    let about = about
        .map(|about| about.trim().to_string())
        .filter(|about| !about.is_empty())
        .map(|about| clip(&about));
    let label = div().min_w_0().child(faded(title.clone()));
    if about.is_none() && key.is_none() {
        return label.into_any_element();
    }
    let muted = cx.theme().muted_foreground;
    h_flex()
        .id(id)
        .min_w_0()
        .gap_1()
        .items_center()
        .child(label)
        .child(
            Icon::new(Lucide::Info)
                .xsmall()
                .flex_none()
                .text_color(muted.opacity(0.7)),
        )
        .tip_element(move |_, cx| {
            let mono = cx.theme().mono_font_family.clone();
            v_flex()
                .max_w(px(ABOUT_WIDTH))
                .gap_1()
                .py_1()
                .child(div().font_medium().child(title.clone()))
                .children(about.clone())
                .children(key.clone().map(|key| {
                    div()
                        .text_xs()
                        .text_color(muted)
                        .font_family(mono)
                        .child(key)
                }))
        })
        .into_any_element()
}

fn clip(text: &str) -> SharedString {
    if text.chars().count() <= ABOUT_CHARS {
        return text.to_string().into();
    }
    let cut: String = text.chars().take(ABOUT_CHARS).collect();
    format!("{}…", cut.trim_end()).into()
}

/// `described` for identifiers: tool, module and skill names.
pub(super) fn named(
    id: ElementId,
    name: impl Into<SharedString>,
    about: Option<SharedString>,
    key: Option<SharedString>,
    cx: &App,
) -> Div {
    div()
        .min_w_0()
        .font_family(cx.theme().mono_font_family.clone())
        .child(described(id, name.into(), about, key, cx))
}

/// Row of a listed item: its name on the left, chips and controls on the
/// right; a narrow window moves them under the name.
pub(super) fn item_row(
    title: impl IntoElement,
    trailing: impl IntoIterator<Item = AnyElement>,
) -> Div {
    field_row(
        title,
        h_flex()
            .flex_wrap()
            .gap_2()
            .items_center()
            .justify_end()
            .children(trailing),
        Fit::Fixed,
    )
}

/// Undo mark beside a title, for values set in the profile.
pub(super) fn reset_mark(
    id: ElementId,
    hint: &str,
    reset: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    kit::button(id)
        .icon(IconName::Undo2)
        .ghost()
        .xsmall()
        .hint(hint, cx)
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            reset(window, cx);
        })
        .into_any_element()
}

#[derive(Clone, Copy)]
pub(super) enum Tone {
    Muted,
    Accent,
    Success,
    Warning,
    Danger,
}

pub(super) fn chip(text: impl Into<SharedString>, tone: Tone, cx: &App) -> Div {
    let theme = cx.theme();
    let color = match tone {
        Tone::Muted => theme.muted_foreground,
        Tone::Accent => theme.primary,
        Tone::Success => theme.success,
        Tone::Warning => theme.warning,
        Tone::Danger => theme.danger,
    };
    div()
        .flex_none()
        .px_1p5()
        .py(px(1.))
        .rounded(theme.radius / 2.)
        .text_xs()
        .text_color(color)
        .bg(color.opacity(0.1))
        .child(text.into())
}

pub(super) fn mono(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .font_family(cx.theme().mono_font_family.clone())
        .child(text.into())
}

/// Monospace label on one line that fades out where it runs out of room.
pub(super) fn mono_line(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .min_w_0()
        .font_family(cx.theme().mono_font_family.clone())
        .child(faded(text))
}

/// Selectable card of a radio group.
pub(super) fn radio_card(
    id: ElementId,
    selected: bool,
    title: impl IntoElement,
    hint: Option<SharedString>,
    chips: Vec<Div>,
    select: impl Fn(&mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let dot = div()
        .mt(px(2.))
        .size(prefs::scaled(16.))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(if selected {
            theme.primary
        } else {
            theme.border
        })
        .flex()
        .items_center()
        .justify_center()
        .when(selected, |dot| {
            dot.child(div().size(px(8.)).rounded_full().bg(theme.primary))
        });
    h_flex()
        .id(id)
        .gap_3()
        .items_start()
        .px_4()
        .py_3()
        .rounded(theme.radius_lg)
        .border_1()
        .cursor_pointer()
        .map(|card| {
            if selected {
                card.border_color(theme.primary.opacity(0.6))
                    .bg(theme.primary.opacity(0.06))
            } else {
                card.border_color(theme.border)
                    .bg(theme.group_box)
                    .hover(|card| card.bg(theme.list_hover))
            }
        })
        .child(dot)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().text_sm().font_medium().child(title))
                .children(hint.map(|hint| {
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(hint)
                }))
                .when(!chips.is_empty(), |text| {
                    text.child(h_flex().flex_wrap().gap_1().children(chips))
                }),
        )
        .on_click(move |_, _, cx| select(cx))
}

/// Row that opens or closes a nested block.
pub(super) fn disclosure(
    id: ElementId,
    open: bool,
    label: impl Into<SharedString>,
    changed: bool,
    toggle: impl Fn(&mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let chevron_id = ElementId::Name(format!("{id}-chevron").into());
    h_flex()
        .id(id)
        .px_4()
        .py_2p5()
        .gap_2()
        .items_center()
        .text_sm()
        .text_color(theme.muted_foreground)
        .cursor_pointer()
        .hover(|row| row.bg(theme.list_hover))
        .child(crate::ui::motion::chevron(
            chevron_id,
            open,
            theme.muted_foreground,
        ))
        .child(label.into())
        .when(changed && !open, |row| row.child(changed_dot(cx)))
        .on_click(move |_, _, cx| toggle(cx))
}

pub(super) struct MenuOption<T> {
    pub label: SharedString,
    pub value: T,
    pub checked: bool,
}

/// Outline button opening a checked list; `select` receives the picked value.
pub(super) fn select_menu<T: Clone + 'static>(
    id: ElementId,
    label: impl Into<SharedString>,
    options: Vec<MenuOption<T>>,
    select: impl Fn(T, &mut App) + 'static,
) -> AnyElement {
    let select = Rc::new(select);
    let options = Rc::new(options);
    kit::button(id)
        .label(label)
        .outline()
        .small()
        .dropdown_caret(true)
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu: PopupMenu, _, _| {
            let menu = menu.min_w(px(200.)).max_h(px(360.)).scrollable(true);
            options.iter().fold(menu, |menu, option| {
                let (select, value) = (select.clone(), option.value.clone());
                menu.item(
                    PopupMenuItem::new(option.label.clone())
                        .checked(option.checked)
                        .on_click(move |_, _, cx| select(value.clone(), cx)),
                )
            })
        })
        .into_any_element()
}

/// Small muted line, skipped when the text is blank.
pub(super) fn hint_text(text: impl Into<String>, cx: &App) -> Option<Div> {
    let text = text.into();
    (!text.trim().is_empty()).then(|| {
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(text)
    })
}

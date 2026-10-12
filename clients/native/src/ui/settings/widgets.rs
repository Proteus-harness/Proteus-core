//! Building blocks of settings pages: sections, cards, rows, chips, radio
//! cards and select menus.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::ui::kit;
use crate::ui::prefs;

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

/// Title and description on the left, the control on the right; wide
/// controls go under the text instead.
pub(super) fn row(
    title: impl IntoElement,
    description: Option<SharedString>,
    control: impl IntoElement,
    vertical: bool,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let text = v_flex()
        .gap_0p5()
        .min_w_0()
        .child(h_flex().gap_2().items_center().text_sm().child(title))
        .children(description.map(|text| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(text)
        }));
    if vertical {
        v_flex()
            .px_4()
            .py_3()
            .gap_2()
            .child(text)
            .child(div().w_full().child(control))
    } else {
        h_flex()
            .px_4()
            .py_3()
            .gap_6()
            .items_center()
            .child(text.flex_1())
            .child(div().flex_none().child(control))
    }
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

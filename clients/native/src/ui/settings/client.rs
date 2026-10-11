//! Client pages: preferences of this device, applied immediately.

use gpui_kit::component::*;
use gpui_kit::*;

use super::SettingsView;
use super::widgets::{MenuOption, card, row, section, select_menu};
use crate::preferences::{MotionPreference, Preferences, SendMode, ThemePreference};
use crate::ui::prefs::{self, ClientPreferences};

const THEMES: [(ThemePreference, &str); 3] = [
    (ThemePreference::System, "Как в системе"),
    (ThemePreference::Dark, "Тёмная"),
    (ThemePreference::Light, "Светлая"),
];

const MOTIONS: [(MotionPreference, &str); 3] = [
    (MotionPreference::Full, "Полные"),
    (MotionPreference::Reduced, "Уменьшенные"),
    (MotionPreference::Off, "Выключены"),
];

const SEND_MODES: [(SendMode, &str); 2] = [
    (SendMode::Enter, "Enter"),
    (SendMode::CtrlEnter, "Ctrl + Enter"),
];

impl SettingsView {
    pub(super) fn appearance_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let current = prefs::current(cx);
        let rows = [
            row(
                "Тема",
                None,
                choice("pref-theme", &THEMES, current.theme, |prefs, value| {
                    prefs.theme = value
                }),
                false,
                cx,
            )
            .into_any_element(),
            row(
                "Анимации",
                Some(
                    "Уменьшенные оставляют плавное появление без сдвигов; выключенные показывают всё сразу."
                        .into(),
                ),
                choice("pref-motion", &MOTIONS, current.motion, |prefs, value| {
                    prefs.motion = value
                }),
                false,
                cx,
            )
            .into_any_element(),
        ];
        let mut parts = self.preferences_problem(cx);
        parts.push(section("Интерфейс", None, None, card(rows, cx), cx).into_any_element());
        parts
    }

    pub(super) fn chat_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let current = prefs::current(cx);
        let rows = [row(
            "Отправка сообщения",
            Some(
                "Shift+Enter всегда переносит строку. Во время ответа отправка добавляет сообщение в очередь."
                    .into(),
            ),
            choice("pref-send", &SEND_MODES, current.send, |prefs, value| {
                prefs.send = value
            }),
            false,
            cx,
        )
        .into_any_element()];
        let mut parts = self.preferences_problem(cx);
        parts.push(section("Поле ввода", None, None, card(rows, cx), cx).into_any_element());
        parts
    }

    fn preferences_problem(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        cx.global::<ClientPreferences>()
            .problem
            .clone()
            .map(|problem| {
                div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(problem)
                    .into_any_element()
            })
            .into_iter()
            .collect()
    }
}

fn choice<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: &[(T, &'static str)],
    current: T,
    apply: fn(&mut Preferences, T),
) -> AnyElement {
    let label = options
        .iter()
        .find(|(value, _)| *value == current)
        .map(|(_, label)| *label)
        .unwrap_or_default();
    let items = options
        .iter()
        .map(|(value, label)| MenuOption {
            label: (*label).into(),
            value: *value,
            checked: *value == current,
        })
        .collect();
    select_menu(id.into(), label, items, move |value, cx| {
        prefs::update(cx, |prefs| apply(prefs, value))
    })
}

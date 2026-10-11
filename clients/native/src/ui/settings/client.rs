//! Client pages: preferences of this device, applied immediately.

use gpui_kit::component::switch::Switch;
use gpui_kit::component::*;
use gpui_kit::*;

use super::SettingsView;
use super::sizes::slider;
use super::widgets::{MenuOption, card, row, section, select_menu};
use crate::preferences::{MotionPreference, Notifications, Preferences, SendMode, ThemePreference};
use crate::ui::prefs::{self, ClientPreferences};
use crate::ui::theme::{self, ThemeProblems};

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

const SCALES: [(u16, &str); 9] = [
    (75, "75%"),
    (80, "80%"),
    (90, "90%"),
    (100, "100%"),
    (110, "110%"),
    (125, "125%"),
    (150, "150%"),
    (175, "175%"),
    (200, "200%"),
];

const SEND_MODES: [(SendMode, &str); 2] = [
    (SendMode::Enter, "Enter"),
    (SendMode::CtrlEnter, "Ctrl + Enter"),
];

const NOTIFICATIONS: [(Notifications, &str); 2] = [
    (Notifications::Background, "Когда окно в фоне"),
    (Notifications::Off, "Выключены"),
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
                "Тёмная палитра",
                None,
                theme_choice("pref-dark-theme", true, &current.themes.dark, cx, |prefs, name| {
                    prefs.themes.dark = name
                }),
                false,
                cx,
            )
            .into_any_element(),
            row(
                "Светлая палитра",
                Some(
                    "Свои темы в формате gpui-kit кладите в ~/.config/dev.proteus.native/themes/; они читаются при запуске."
                        .into(),
                ),
                theme_choice("pref-light-theme", false, &current.themes.light, cx, |prefs, name| {
                    prefs.themes.light = name
                }),
                false,
                cx,
            )
            .into_any_element(),
            row(
                "Масштаб",
                Some("Ctrl + = и Ctrl + − меняют масштаб, Ctrl + 0 возвращает 100%.".into()),
                choice("pref-scale", &SCALES, current.view.scale, |prefs, value| {
                    prefs.view.scale = value
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
        parts.extend(cx.global::<ThemeProblems>().0.iter().map(|problem| {
            div()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(format!("Тема не прочитана: {problem}"))
                .into_any_element()
        }));
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
        let sizes = [
            row(
                "Размер текста",
                Some("Сообщения диалога и поле ввода.".into()),
                slider(&self.sizes.chat_text, "px", cx),
                false,
                cx,
            )
            .into_any_element(),
            row(
                "Ширина диалога",
                Some("Наибольшая ширина ленты и поля ввода.".into()),
                slider(&self.sizes.chat_width, "px", cx),
                false,
                cx,
            )
            .into_any_element(),
            row(
                "Компактные цепочки инструментов",
                Some("Три и больше вызова подряд сворачиваются в одну строку.".into()),
                Switch::new("pref-compact-tools")
                    .checked(current.view.compact_tools)
                    .on_click(|checked, _, cx| {
                        let checked = *checked;
                        prefs::update(cx, move |prefs| prefs.view.compact_tools = checked)
                    })
                    .into_any_element(),
                false,
                cx,
            )
            .into_any_element(),
            row(
                "Автопрокрутка",
                Some("Плавно следовать за ответом, пока лента внизу.".into()),
                Switch::new("pref-autoscroll")
                    .checked(current.view.autoscroll)
                    .on_click(|checked, _, cx| {
                        let checked = *checked;
                        prefs::update(cx, move |prefs| prefs.view.autoscroll = checked)
                    })
                    .into_any_element(),
                false,
                cx,
            )
            .into_any_element(),
            row(
                "Уведомления",
                Some("Когда окно в фоне: агент закончил, ждёт подтверждения или ответа.".into()),
                choice(
                    "pref-notifications",
                    &NOTIFICATIONS,
                    current.notifications,
                    |prefs, value| prefs.notifications = value,
                ),
                false,
                cx,
            )
            .into_any_element(),
        ];
        let mut parts = self.preferences_problem(cx);
        parts.push(section("Диалог", None, None, card(sizes, cx), cx).into_any_element());
        parts.push(section("Поле ввода", None, None, card(rows, cx), cx).into_any_element());
        parts
    }

    pub(super) fn preferences_problem(&self, cx: &Context<Self>) -> Vec<AnyElement> {
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

fn theme_choice(
    id: &'static str,
    dark: bool,
    current: &str,
    cx: &App,
    apply: fn(&mut Preferences, String),
) -> AnyElement {
    let items = theme::names(dark, cx)
        .into_iter()
        .map(|name| MenuOption {
            label: theme::label(&name),
            checked: name.as_ref() == current,
            value: name,
        })
        .collect();
    select_menu(id.into(), theme::label(current), items, move |name, cx| {
        prefs::update(cx, |prefs| apply(prefs, name.to_string()))
    })
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

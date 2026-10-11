//! Client preferences in the running app: a global plus the effects of each
//! setting (theme, motion level).

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

use crate::preferences::{self, MotionPreference, Preferences, ThemePreference};

const BASE_FONT: f32 = 16.;
const BASE_MONO_FONT: f32 = 13.;

/// Interface scale: 1.0 at 100%.
pub fn scale(cx: &App) -> f32 {
    current(cx).view.scale_factor()
}

/// A length given in px at 100% scale; follows the interface scale.
pub fn scaled(value: f32) -> Rems {
    rems(value / BASE_FONT)
}

/// Text size of chat messages and the composer.
pub fn chat_text(cx: &App) -> Rems {
    scaled(f32::from(current(cx).view.chat_text))
}

/// Widest conversation column.
pub fn chat_width(cx: &App) -> Rems {
    scaled(f32::from(current(cx).view.chat_width))
}

pub struct ClientPreferences {
    pub value: Preferences,
    /// Why the stored file was not used; shown on the client settings pages.
    pub problem: Option<String>,
}

impl Global for ClientPreferences {}

pub fn init(cx: &mut App) {
    let (value, problem) = match preferences::load() {
        Ok(value) => (value, None),
        Err(error) => (Preferences::default(), Some(format!("{error:#}"))),
    };
    apply(&value, None, cx);
    cx.set_global(ClientPreferences { value, problem });
}

pub fn current(cx: &App) -> &Preferences {
    &cx.global::<ClientPreferences>().value
}

/// Applies the change right away and stores it on this device.
pub fn update(cx: &mut App, change: impl FnOnce(&mut Preferences)) {
    let previous = current(cx).clone();
    let mut value = previous.clone();
    change(&mut value);
    if value == previous {
        return;
    }
    apply(&value, Some(&previous), cx);
    let problem = preferences::save(&value)
        .err()
        .map(|error| format!("Не сохранено: {error:#}"));
    let global = cx.global_mut::<ClientPreferences>();
    global.value = value;
    global.problem = problem;
}

fn apply(value: &Preferences, previous: Option<&Preferences>, cx: &mut App) {
    if previous.is_none_or(|previous| previous.theme != value.theme) {
        match value.theme {
            ThemePreference::System => Theme::sync_system_appearance(None, cx),
            ThemePreference::Dark => Theme::change(ThemeMode::Dark, None, cx),
            ThemePreference::Light => Theme::change(ThemeMode::Light, None, cx),
        }
    }
    // The root sets the window's rem from the theme font size, so this
    // scales every rem-based size of the interface.
    let scale = value.view.scale_factor();
    if Theme::global(cx).font_size != px(BASE_FONT * scale) {
        Theme::update(cx, |theme| {
            theme.font_size = px(BASE_FONT * scale);
            theme.mono_font_size = px(BASE_MONO_FONT * scale);
        });
    }
    super::motion::set_level(value.motion);
    cx.set_reduce_motion(value.motion == MotionPreference::Off);
    cx.refresh_windows();
}

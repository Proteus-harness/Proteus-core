//! Client preferences in the running app: a global plus the effects of each
//! setting (theme, motion level).

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

use crate::preferences::{self, MotionPreference, Preferences, ThemePreference};

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
    apply(&value, cx);
    cx.set_global(ClientPreferences { value, problem });
}

pub fn current(cx: &App) -> &Preferences {
    &cx.global::<ClientPreferences>().value
}

/// Applies the change right away and stores it on this device.
pub fn update(cx: &mut App, change: impl FnOnce(&mut Preferences)) {
    let mut value = current(cx).clone();
    change(&mut value);
    apply(&value, cx);
    let problem = preferences::save(&value)
        .err()
        .map(|error| format!("Не сохранено: {error:#}"));
    let global = cx.global_mut::<ClientPreferences>();
    global.value = value;
    global.problem = problem;
}

fn apply(value: &Preferences, cx: &mut App) {
    match value.theme {
        ThemePreference::System => Theme::sync_system_appearance(None, cx),
        ThemePreference::Dark => Theme::change(ThemeMode::Dark, None, cx),
        ThemePreference::Light => Theme::change(ThemeMode::Light, None, cx),
    }
    super::motion::set_level(value.motion);
    cx.set_reduce_motion(value.motion == MotionPreference::Off);
    cx.refresh_windows();
}

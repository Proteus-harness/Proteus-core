//! Colour themes: the bundled Proteus set and the user's theme files in
//! `<config>/dev.proteus.native/themes/*.json` (the gpui-kit theme format),
//! read once at start. The preferences name one theme per appearance.

use std::rc::Rc;

use gpui_kit::component::{Theme, ThemeConfig, ThemeMode, ThemeRegistry};
use gpui_kit::*;

use crate::preferences::{self, DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME, ThemeNames};

const BUNDLED: &str = include_str!("proteus.json");

/// Theme files that could not be read; shown on the appearance page.
#[derive(Default)]
pub struct ThemeProblems(pub Vec<String>);

impl Global for ThemeProblems {}

pub(super) fn init(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    registry
        .load_themes_from_str(BUNDLED)
        .expect("bundled themes are valid");
    let mut problems = Vec::new();
    if let Some(dir) = preferences::config_dir().map(|dir| dir.join("themes"))
        && let Ok(entries) = std::fs::read_dir(&dir)
    {
        let mut files: Vec<_> = entries
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        files.sort();
        for file in files {
            let loaded = std::fs::read_to_string(&file)
                .map_err(anyhow::Error::from)
                .and_then(|text| registry.load_themes_from_str(&text));
            if let Err(error) = loaded {
                problems.push(format!("{}: {error:#}", file.display()));
            }
        }
    }
    cx.set_global(ThemeProblems(problems));
}

/// Names of the registered themes of one appearance, the default first.
pub(super) fn names(dark: bool, cx: &App) -> Vec<SharedString> {
    let fallback = if dark {
        DEFAULT_DARK_THEME
    } else {
        DEFAULT_LIGHT_THEME
    };
    let mut names: Vec<SharedString> = ThemeRegistry::global(cx)
        .sorted_themes()
        .into_iter()
        .filter(|theme| theme.mode.is_dark() == dark)
        .map(|theme| theme.name.clone())
        .collect();
    if let Some(at) = names.iter().position(|name| name == fallback) {
        let name = names.remove(at);
        names.insert(0, name);
    }
    names
}

/// Installs the named themes for both appearances; a missing name falls
/// back to the bundled theme. The caller then applies the current mode.
pub(super) fn select(names: &ThemeNames, cx: &mut App) {
    let dark = pick(&names.dark, ThemeMode::Dark, DEFAULT_DARK_THEME, cx);
    let light = pick(&names.light, ThemeMode::Light, DEFAULT_LIGHT_THEME, cx);
    let theme = Theme::global_mut(cx);
    if let Some(dark) = dark {
        theme.dark_theme = dark;
    }
    if let Some(light) = light {
        theme.light_theme = light;
    }
}

fn pick(name: &str, mode: ThemeMode, fallback: &str, cx: &App) -> Option<Rc<ThemeConfig>> {
    let themes = ThemeRegistry::global(cx).themes();
    themes
        .get(name)
        .filter(|theme| theme.mode == mode)
        .or_else(|| themes.get(fallback))
        .cloned()
}

/// Russian names of the themes that come with the client.
pub(super) fn label(name: &str) -> SharedString {
    match name {
        "Proteus Graphite" => "Графит".into(),
        "Proteus Light" => "Светлая".into(),
        "Default Dark" => "Чёрная".into(),
        "Default Light" => "Белая".into(),
        other => SharedString::from(other.to_owned()),
    }
}

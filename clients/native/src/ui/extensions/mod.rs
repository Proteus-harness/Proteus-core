//! Client extensions: compact icons, panels and document tabs.
//!
//! Every extension is described by a [`Manifest`] and talks to the client
//! only through [`Host`], so built-in extensions follow the same contract a
//! separately shipped one would. Users switch extensions on and off and
//! arrange their icons; the choice is a client preference.

mod badge;
mod declarative;
mod host;
pub(in crate::ui) mod session;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::*;
use std::path::Path;

pub(super) use badge::DraggedBadge;
pub(super) use host::{Host, HostEvent, OpenDocument};

use super::prefs;
use crate::declarative::manifest::Problem;
use crate::preferences::BadgePlace;

/// Builds an icon view. Runs inside a workspace update, so it must not read
/// the [`Host`] synchronously.
pub(super) type BuildView = fn(Host, &mut Window, &mut App) -> AnyView;

/// Builds a panel; same rules as [`BuildView`].
pub(super) type BuildFocused = fn(Host, &mut Window, &mut App) -> SurfaceView;

/// Builds the view of one document, e.g. a file; same rules as [`BuildView`].
pub(super) type BuildDocument = fn(Host, &str, &mut Window, &mut App) -> SurfaceView;

/// Panel shown in the side panel beside the rail, like the session list.
pub(super) struct PanelSurface {
    pub title: &'static str,
    pub build: BuildFocused,
}

/// Documents opened as tabs with ids `<extension>:<argument>`.
pub(super) struct DocumentSurface {
    pub build: BuildDocument,
    /// Short tab title of an argument.
    pub label: Option<&'static str>,
    pub command: Option<&'static str>,
}

/// Compact icon and where it goes until the user moves it.
pub(super) struct BadgeSurface {
    pub build: BuildView,
    pub place: BadgePlace,
    /// No wider than a plain icon, so it fits the rail.
    pub narrow: bool,
}

/// A built view and the focus it takes when shown, so its own keyboard
/// shortcuts work right away.
#[derive(Clone)]
pub(super) struct SurfaceView {
    pub view: AnyView,
    pub focus: FocusHandle,
}

pub(super) struct Manifest {
    pub id: &'static str,
    pub title: &'static str,
    pub author: &'static str,
    pub description: &'static str,
    pub icon: Lucide,
    /// Read-only analytics page among the extension pages of the settings.
    pub page: Option<BuildFocused>,
    pub panel: Option<PanelSurface>,
    pub document: Option<DocumentSurface>,
    /// Icon under the composer or in a window bar.
    pub badge: Option<BadgeSurface>,
}

/// Shipped packages, overridden by user installations with the same id. The extensions folder
/// is read on first use and again after an install or removal.
pub(super) fn catalog() -> Vec<&'static Manifest> {
    declarative::manifests()
}

/// Installed extension folders that failed to load, and where they live.
pub(super) fn problems() -> Vec<Problem> {
    declarative::problems()
}

/// Re-reads managed folders after a catalog installation or removal.
pub(super) fn reload_installed(cx: &mut App) {
    declarative::rescan();
    declarative::reload_runtimes(cx);
    cx.refresh_windows();
}

/// Folder of an extension installed by the user; built-in ones have none.
pub(super) fn folder(id: &str) -> Option<&'static Path> {
    declarative::folder(id)
}

/// Unpacks an extension archive into the extensions folder and adds it to
/// the catalog right away. Returns the new extension.
pub(super) fn install(archive: &Path, cx: &mut App) -> anyhow::Result<&'static Manifest> {
    let root = crate::declarative::manifest::root()
        .ok_or_else(|| anyhow::anyhow!("Не найдена папка настроек"))?;
    let manifest = crate::declarative::package::install(&root, archive, &[])?;
    declarative::rescan();
    declarative::reload_runtimes(cx);
    cx.refresh_windows();
    find(&manifest.id).ok_or_else(|| anyhow::anyhow!("Расширение `{}` не загрузилось", manifest.id))
}

/// Removes a user package. A restored shipped version stays disabled.
pub(super) fn remove(id: &str, cx: &mut App) -> anyhow::Result<()> {
    let root = crate::declarative::manifest::root()
        .ok_or_else(|| anyhow::anyhow!("Не найдена папка настроек"))?;
    crate::declarative::package::remove(&root, id)?;
    declarative::rescan();
    declarative::reload_runtimes(cx);
    prefs::update(cx, |preferences| {
        preferences.extensions.forget(id);
        if find(id).is_some() {
            preferences.extensions.set_enabled(id, false);
        }
    });
    cx.refresh_windows();
    Ok(())
}

pub(super) fn find(id: &str) -> Option<&'static Manifest> {
    catalog().into_iter().find(|manifest| manifest.id == id)
}

pub(super) fn is_enabled(id: &str, cx: &App) -> bool {
    prefs::current(cx).extensions.is_enabled(id)
}

/// Every extension in the user's order.
pub(super) fn ordered(cx: &App) -> Vec<&'static Manifest> {
    let all = catalog();
    let ids: Vec<&'static str> = all.iter().map(|manifest| manifest.id).collect();
    prefs::current(cx)
        .extensions
        .ordered(&ids)
        .into_iter()
        .filter_map(|id| all.iter().find(|manifest| manifest.id == id).copied())
        .collect()
}

/// Ids of every extension, in catalog order.
pub(super) fn ids() -> Vec<&'static str> {
    catalog().iter().map(|manifest| manifest.id).collect()
}

/// Ids of extensions with an icon, in catalog order.
pub(super) fn badge_ids() -> Vec<&'static str> {
    catalog()
        .iter()
        .filter(|manifest| manifest.badge.is_some())
        .map(|manifest| manifest.id)
        .collect()
}

pub(super) fn badge_place(manifest: &Manifest, cx: &App) -> BadgePlace {
    let default = manifest
        .badge
        .as_ref()
        .map_or(BadgePlace::Hidden, |badge| badge.place);
    prefs::current(cx)
        .extensions
        .badge_place(manifest.id, default)
}

/// Whether the icon of `manifest` can be put into `place`.
pub(super) fn fits(place: BadgePlace, manifest: &Manifest) -> bool {
    !place.is_rail() || manifest.badge.as_ref().is_some_and(|badge| badge.narrow)
}

/// Enabled icons shown in `place`, in the user's order.
pub(super) fn visible_badges(place: BadgePlace, cx: &App) -> Vec<&'static Manifest> {
    let extensions = &prefs::current(cx).extensions;
    extensions
        .badges_ordered(&badge_ids())
        .into_iter()
        .filter(|id| extensions.is_enabled(id))
        .filter_map(find)
        .filter(|manifest| badge_place(manifest, cx) == place)
        .collect()
}

/// Enabled extensions with a panel, in catalog order.
pub(super) fn visible_panels(cx: &App) -> Vec<&'static Manifest> {
    catalog()
        .into_iter()
        .filter(|manifest| manifest.panel.is_some() && is_enabled(manifest.id, cx))
        .collect()
}

/// Panel surface of an enabled extension.
pub(super) fn panel(id: &str, cx: &App) -> Option<&'static Manifest> {
    find(id).filter(|manifest| manifest.panel.is_some() && is_enabled(manifest.id, cx))
}

/// Document surface of an enabled extension for a tab id.
pub(super) fn document(tab: &str, cx: &App) -> Option<(&'static Manifest, String)> {
    let (extension, argument) = crate::tabs::split_document(tab)?;
    let manifest = find(extension).filter(|manifest| is_enabled(manifest.id, cx))?;
    manifest.document.as_ref()?;
    Some((manifest, argument.to_string()))
}

#[cfg(test)]
mod tests {
    use crate::preferences::SESSIONS_PANEL;
    use crate::tabs;

    #[test]
    fn documents_never_look_like_chats_and_panels_not_like_sessions() {
        for manifest in super::catalog() {
            let id = tabs::document(manifest.id, "x");
            assert!(!tabs::is_chat(&id), "extension `{}`", manifest.id);
            assert_ne!(manifest.id, SESSIONS_PANEL);
        }
    }
}

pub(super) fn next_document(
    command: &str,
    layout: &crate::tabs::TabLayout,
    cx: &App,
) -> Option<String> {
    let manifest = ordered(cx).into_iter().find(|manifest| {
        is_enabled(manifest.id, cx)
            && manifest
                .document
                .as_ref()
                .is_some_and(|document| document.command == Some(command))
    })?;
    let prefix = format!("{}:", manifest.id);
    let last = layout
        .panes()
        .iter()
        .flat_map(|pane| &pane.tabs)
        .filter_map(|tab| tab.strip_prefix(&prefix)?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    Some(format!("{prefix}{}", last + 1))
}
impl DocumentSurface {
    pub(super) fn title(&self, argument: &str) -> SharedString {
        self.label
            .unwrap_or("{argument}")
            .replace("{argument}", argument)
            .replace(
                "{filename}",
                argument.rsplit('/').next().unwrap_or(argument),
            )
            .into()
    }
}

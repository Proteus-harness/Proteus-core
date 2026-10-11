//! Client extensions: compact icons, left sidebar views and document tabs.
//!
//! Every extension is described by a [`Manifest`] and talks to the client
//! only through [`Host`], so built-in extensions follow the same contract a
//! separately shipped one would. Users switch extensions on and off and
//! arrange their icons; the choice is a client preference.

mod badge;
mod files;
mod host;
mod quota;
mod session_badges;
mod session_info;
mod shared;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, IconName};
use gpui_kit::*;

pub(super) use badge::DraggedBadge;
pub(super) use host::{Host, HostEvent, OpenDocument};

use super::prefs;
use crate::preferences::BadgePlace;

/// Builds an icon view. Runs inside a workspace update, so it must not read
/// the [`Host`] synchronously.
pub(super) type BuildView = fn(Host, &mut Window, &mut App) -> AnyView;

/// Builds a sidebar view; same rules as [`BuildView`].
pub(super) type BuildFocused = fn(Host, &mut Window, &mut App) -> SurfaceView;

/// Builds the view of one document, e.g. a file; same rules as [`BuildView`].
pub(super) type BuildDocument = fn(Host, &str, &mut Window, &mut App) -> SurfaceView;

/// View in the left sidebar, next to the session list.
pub(super) struct SidebarSurface {
    pub title: &'static str,
    pub build: BuildFocused,
}

/// Documents opened as tabs with ids `<extension>:<argument>`.
pub(super) struct DocumentSurface {
    pub build: BuildDocument,
    /// Short tab title of an argument.
    pub label: fn(&str) -> SharedString,
}

/// Compact icon and where it goes until the user moves it.
pub(super) struct BadgeSurface {
    pub build: BuildView,
    pub place: BadgePlace,
}

/// A built view and the focus it takes when shown, so its own keyboard
/// shortcuts work right away.
#[derive(Clone)]
pub(super) struct SurfaceView {
    pub view: AnyView,
    pub focus: FocusHandle,
}

/// Key bindings of extensions; registered once at startup.
pub(super) fn bind_keys(cx: &mut App) {
    files::bind_keys(cx);
}

pub(super) struct Manifest {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub icon: fn() -> Icon,
    pub sidebar: Option<SidebarSurface>,
    pub document: Option<DocumentSurface>,
    /// Icon under the composer or in the tab bar.
    pub badge: Option<BadgeSurface>,
}

pub(super) static CATALOG: [Manifest; 6] = [
    Manifest {
        id: session_info::CONTEXT,
        title: "Контекст",
        description: "Заполненность контекста модели и порог автокомпакта.",
        icon: || Icon::new(Lucide::Layers),
        sidebar: None,
        document: None,
        badge: Some(BadgeSurface {
            build: session_badges::build_context,
            place: BadgePlace::Composer,
        }),
    },
    Manifest {
        id: session_info::PLAN,
        title: "План",
        description: "Шаги текущего плана агента.",
        icon: || Icon::new(Lucide::ListChecks),
        sidebar: None,
        document: None,
        badge: Some(BadgeSurface {
            build: session_badges::build_plan,
            place: BadgePlace::Composer,
        }),
    },
    Manifest {
        id: quota::ID,
        title: "Лимиты модели",
        description: "Остаток лимитов у провайдера модели.",
        icon: || Icon::new(Lucide::Gauge),
        sidebar: None,
        document: None,
        badge: Some(BadgeSurface {
            build: quota::build_badge,
            place: BadgePlace::Composer,
        }),
    },
    Manifest {
        id: files::ID,
        title: "Файлы",
        description: "Дерево рабочей папки и изменения git в боковой панели, файлы во вкладках.",
        icon: || Icon::new(Lucide::FolderTree),
        sidebar: Some(SidebarSurface {
            title: "Файлы",
            build: files::build_sidebar,
        }),
        document: Some(DocumentSurface {
            build: files::build_document,
            label: files::document_label,
        }),
        badge: Some(BadgeSurface {
            build: files::build_badge,
            place: BadgePlace::Header,
        }),
    },
    Manifest {
        id: session_info::SESSION,
        title: "Сессия",
        description: "Статус, число сообщений и ожидающие запросы.",
        icon: || Icon::new(IconName::Info),
        sidebar: None,
        document: None,
        badge: Some(BadgeSurface {
            build: session_badges::build_session,
            place: BadgePlace::Hidden,
        }),
    },
    Manifest {
        id: session_info::AGENT,
        title: "Агент",
        description: "Профиль, модель, режим и инструменты сессии.",
        icon: || Icon::new(IconName::Bot),
        sidebar: None,
        document: None,
        badge: Some(BadgeSurface {
            build: session_badges::build_agent,
            place: BadgePlace::Header,
        }),
    },
];

pub(super) fn find(id: &str) -> Option<&'static Manifest> {
    CATALOG.iter().find(|manifest| manifest.id == id)
}

pub(super) fn is_enabled(id: &str, cx: &App) -> bool {
    prefs::current(cx).extensions.is_enabled(id)
}

/// Ids of extensions with an icon, in catalog order.
pub(super) fn badge_ids() -> Vec<&'static str> {
    CATALOG
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

/// Enabled extensions with a sidebar view, in catalog order.
pub(super) fn visible_sidebars(cx: &App) -> Vec<&'static Manifest> {
    CATALOG
        .iter()
        .filter(|manifest| manifest.sidebar.is_some() && is_enabled(manifest.id, cx))
        .collect()
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
    #[test]
    fn documents_never_look_like_chats() {
        for manifest in &super::CATALOG {
            let id = crate::tabs::document(manifest.id, "x");
            assert!(!crate::tabs::is_chat(&id), "extension `{}`", manifest.id);
        }
    }
}

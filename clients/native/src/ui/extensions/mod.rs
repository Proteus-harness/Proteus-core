//! Client extensions: compact icons, side panel widgets and tabs.
//!
//! Every extension is described by a [`Manifest`] and talks to the client
//! only through [`Host`], so built-in extensions follow the same contract a
//! separately shipped one would. Users switch extensions on and off and
//! arrange their icons and panel widgets; the choice is a client preference.

mod badge;
mod files;
mod frame;
mod host;
mod notes;
mod quota;
mod session_badges;
mod session_info;
mod shared;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, IconName};
use gpui_kit::*;

pub(super) use badge::DraggedBadge;
pub(super) use frame::{DraggedWidget, drag_preview};
pub(super) use host::{Host, HostEvent};

use super::prefs;
use crate::preferences::BadgePlace;

/// Builds a surface view. Runs inside a workspace update, so it must not
/// read the [`Host`] synchronously.
pub(super) type BuildView = fn(Host, &mut Window, &mut App) -> AnyView;

/// Builds a page; same rules as [`BuildView`].
pub(super) type BuildPage = fn(Host, &mut Window, &mut App) -> PageView;

pub(super) struct Surface {
    pub title: &'static str,
    pub build: BuildView,
}

pub(super) struct PageSurface {
    pub title: &'static str,
    pub build: BuildPage,
}

/// Compact icon and where it goes until the user moves it.
pub(super) struct BadgeSurface {
    pub build: BuildView,
    pub place: BadgePlace,
}

/// A built page and the focus it takes when shown, so its own keyboard
/// shortcuts work right away.
#[derive(Clone)]
pub(super) struct PageView {
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
    /// Widget in the side panel.
    pub panel: Option<Surface>,
    /// Page opened as a tab.
    pub page: Option<PageSurface>,
    /// Icon under the composer or in the tab bar.
    pub badge: Option<BadgeSurface>,
}

pub(super) static CATALOG: [Manifest; 7] = [
    Manifest {
        id: session_info::CONTEXT,
        title: "Контекст",
        description: "Заполненность контекста модели и порог автокомпакта.",
        icon: || Icon::new(Lucide::Layers),
        panel: Some(Surface {
            title: "Контекст",
            build: session_info::build_context,
        }),
        page: None,
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
        panel: Some(Surface {
            title: "План",
            build: session_info::build_plan,
        }),
        page: None,
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
        panel: Some(Surface {
            title: "Лимиты модели",
            build: quota::build,
        }),
        page: None,
        badge: Some(BadgeSurface {
            build: quota::build_badge,
            place: BadgePlace::Composer,
        }),
    },
    Manifest {
        id: files::ID,
        title: "Файлы",
        description: "Дерево рабочей папки, просмотр файлов и изменения git.",
        icon: || Icon::new(Lucide::FolderTree),
        panel: Some(Surface {
            title: "Изменения",
            build: files::build_changes,
        }),
        page: Some(PageSurface {
            title: "Файлы",
            build: files::build_page,
        }),
        badge: Some(BadgeSurface {
            build: files::build_badge,
            place: BadgePlace::Header,
        }),
    },
    Manifest {
        id: notes::ID,
        title: "Заметки",
        description: "Личные заметки, общие для всех рабочих папок.",
        icon: || Icon::new(Lucide::StickyNote),
        panel: None,
        page: Some(PageSurface {
            title: "Заметки",
            build: notes::build,
        }),
        badge: Some(BadgeSurface {
            build: notes::build_badge,
            place: BadgePlace::Header,
        }),
    },
    Manifest {
        id: session_info::SESSION,
        title: "Сессия",
        description: "Статус, число сообщений и ожидающие запросы.",
        icon: || Icon::new(IconName::Info),
        panel: Some(Surface {
            title: "Сессия",
            build: session_info::build_session,
        }),
        page: None,
        badge: Some(BadgeSurface {
            build: session_badges::build_session,
            place: BadgePlace::Hidden,
        }),
    },
    Manifest {
        id: session_info::AGENT,
        title: "Агент",
        description: "Профиль, модель, режим и модули сессии.",
        icon: || Icon::new(IconName::Bot),
        panel: Some(Surface {
            title: "Агент",
            build: session_info::build_agent,
        }),
        page: None,
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

/// Ids of extensions with a panel widget, in catalog order.
pub(super) fn panel_ids() -> Vec<&'static str> {
    CATALOG
        .iter()
        .filter(|manifest| manifest.panel.is_some())
        .map(|manifest| manifest.id)
        .collect()
}

/// Enabled panel widgets in the user's order.
pub(super) fn visible_panels(cx: &App) -> Vec<&'static Manifest> {
    let extensions = &prefs::current(cx).extensions;
    extensions
        .ordered(&panel_ids())
        .into_iter()
        .filter(|id| extensions.is_enabled(id))
        .filter_map(find)
        .collect()
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

/// Enabled extensions with a page, in catalog order.
pub(super) fn visible_pages(cx: &App) -> Vec<&'static Manifest> {
    CATALOG
        .iter()
        .filter(|manifest| manifest.page.is_some() && is_enabled(manifest.id, cx))
        .collect()
}

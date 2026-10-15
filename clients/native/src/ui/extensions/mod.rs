//! Client extensions: compact icons, panels and document tabs.
//!
//! Every extension is described by a [`Manifest`] and talks to the client
//! only through [`Host`], so built-in extensions follow the same contract a
//! separately shipped one would. Users switch extensions on and off and
//! arrange their icons; the choice is a client preference.

mod badge;
mod declarative;
mod files;
mod host;
mod quota;
mod selectors;
mod session_badges;
mod session_info;
mod shared;
mod terminal;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::*;
use std::path::Path;
use std::sync::OnceLock;

pub(super) use badge::DraggedBadge;
pub(super) use host::{Host, HostEvent, OpenDocument};
pub(super) use terminal::next_tab as next_terminal;

use super::prefs;
use crate::declarative::manifest::Problem;
use crate::preferences::BadgePlace;
use crate::settings::catalog::Overview;

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
    pub label: fn(&str) -> SharedString,
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

/// Key bindings of extensions; registered once at startup.
pub(super) fn bind_keys(cx: &mut App) {
    files::bind_keys(cx);
}

pub(super) struct Manifest {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub icon: Lucide,
    /// Read-only analytics page among the extension pages of the settings.
    pub page: Option<Overview>,
    pub panel: Option<PanelSurface>,
    pub document: Option<DocumentSurface>,
    /// Icon under the composer or in a window bar.
    pub badge: Option<BadgeSurface>,
}

fn builtin() -> [Manifest; 12] {
    [
        selector(
            selectors::MODE,
            "Режим",
            "Выбор режима разрешений сессии.",
            Lucide::ShieldCheck,
            selectors::build_mode,
        ),
        selector(
            selectors::MODEL,
            "Модель",
            "Выбор модели сессии и глубины её рассуждений.",
            Lucide::Cpu,
            selectors::build_model,
        ),
        Manifest {
            id: session_info::CONTEXT,
            title: "Контекст",
            description: "Заполненность контекста модели и порог автокомпакта.",
            icon: Lucide::Layers,
            page: None,
            panel: None,
            document: None,
            badge: Some(BadgeSurface {
                build: session_badges::build_context,
                place: BadgePlace::RailEnd,
                narrow: true,
            }),
        },
        Manifest {
            id: session_info::PLAN,
            title: "План",
            description: "Шаги текущего плана агента.",
            icon: Lucide::ListChecks,
            page: None,
            panel: None,
            document: None,
            badge: Some(BadgeSurface {
                build: session_badges::build_plan,
                place: BadgePlace::ComposerStart,
                narrow: false,
            }),
        },
        Manifest {
            id: quota::ID,
            title: "Лимиты модели",
            description: "Остаток лимитов у провайдера модели.",
            icon: Lucide::Gauge,
            page: None,
            panel: None,
            document: None,
            badge: Some(BadgeSurface {
                build: quota::build_badge,
                place: BadgePlace::ComposerStart,
                narrow: true,
            }),
        },
        Manifest {
            id: files::ID,
            title: "Файлы",
            description: "Дерево рабочей папки и изменения git на панели, файлы во вкладках.",
            icon: Lucide::FolderTree,
            page: None,
            panel: Some(PanelSurface {
                title: "Файлы",
                build: files::build_panel,
            }),
            document: Some(DocumentSurface {
                build: files::build_document,
                label: files::document_label,
            }),
            badge: Some(BadgeSurface {
                build: files::build_badge,
                place: BadgePlace::HeaderEnd,
                narrow: true,
            }),
        },
        Manifest {
            id: session_info::SESSION,
            title: "Сессия",
            description: "Статус, число сообщений и ожидающие запросы.",
            icon: Lucide::Info,
            page: None,
            panel: None,
            document: None,
            badge: Some(BadgeSurface {
                build: session_badges::build_session,
                place: BadgePlace::RailEnd,
                narrow: true,
            }),
        },
        Manifest {
            id: session_info::AGENT,
            title: "Агент",
            description: "Профиль, модель, режим и инструменты сессии.",
            icon: Lucide::Bot,
            page: None,
            panel: None,
            document: None,
            badge: Some(BadgeSurface {
                build: session_badges::build_agent,
                place: BadgePlace::RailEnd,
                narrow: true,
            }),
        },
        Manifest {
            id: terminal::ID,
            title: "Терминал",
            description: "Оболочка в рабочей папке во вкладке.",
            icon: Lucide::SquareTerminal,
            page: None,
            panel: None,
            document: Some(DocumentSurface {
                build: terminal::build_document,
                label: terminal::document_label,
            }),
            badge: None,
        },
        analytics(
            "usage",
            "Расход и контекст",
            "Токены, стоимость и заполнение контекста по запросам сессии.",
            Lucide::ChartColumn,
            Overview::Usage,
        ),
        analytics(
            "analysis",
            "Анализ ходов",
            "Шаги ходов агента: запросы модели, инструменты и хуки.",
            Lucide::Activity,
            Overview::Analysis,
        ),
        analytics(
            "architecture",
            "Архитектура",
            "Карта модулей и слотов запущенного агента.",
            Lucide::Network,
            Overview::Architecture,
        ),
    ]
}

/// A read-only analytics page in the settings.
fn analytics(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    icon: Lucide,
    page: Overview,
) -> Manifest {
    Manifest {
        id,
        title,
        description,
        icon,
        page: Some(page),
        panel: None,
        document: None,
        badge: None,
    }
}

/// A session setting selector, kept under the composer until moved.
fn selector(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    icon: Lucide,
    build: BuildView,
) -> Manifest {
    Manifest {
        id,
        title,
        description,
        icon,
        page: None,
        panel: None,
        document: None,
        badge: Some(BadgeSurface {
            build,
            place: BadgePlace::ComposerStart,
            narrow: false,
        }),
    }
}

fn builtins() -> &'static [Manifest] {
    static BUILTIN: OnceLock<[Manifest; 12]> = OnceLock::new();
    BUILTIN.get_or_init(builtin)
}

fn builtin_ids() -> Vec<&'static str> {
    builtins().iter().map(|manifest| manifest.id).collect()
}

/// Built-in extensions followed by the installed ones. The extensions folder
/// is read on first use and again after an install or removal.
pub(super) fn catalog() -> Vec<&'static Manifest> {
    let taken = builtin_ids();
    builtins()
        .iter()
        .chain(declarative::manifests(&taken))
        .collect()
}

/// Installed extension folders that failed to load, and where they live.
pub(super) fn problems() -> Vec<Problem> {
    declarative::problems(&builtin_ids())
}

/// Re-reads managed folders after a catalog installation or removal.
pub(super) fn reload_installed(cx: &mut App) {
    declarative::rescan(&builtin_ids());
    cx.refresh_windows();
}

/// Folder of an extension installed by the user; built-in ones have none.
pub(super) fn folder(id: &str) -> Option<&'static Path> {
    declarative::folder(&builtin_ids(), id)
}

/// Unpacks an extension archive into the extensions folder and adds it to
/// the catalog right away. Returns the new extension.
pub(super) fn install(archive: &Path, cx: &mut App) -> anyhow::Result<&'static Manifest> {
    let root = crate::declarative::manifest::root()
        .ok_or_else(|| anyhow::anyhow!("Не найдена папка настроек"))?;
    let taken = builtin_ids();
    let manifest = crate::declarative::package::install(&root, archive, &taken)?;
    declarative::rescan(&taken);
    cx.refresh_windows();
    find(&manifest.id).ok_or_else(|| anyhow::anyhow!("Расширение `{}` не загрузилось", manifest.id))
}

/// Deletes an installed extension with its folder and forgets its settings.
pub(super) fn remove(id: &str, cx: &mut App) -> anyhow::Result<()> {
    let root = crate::declarative::manifest::root()
        .ok_or_else(|| anyhow::anyhow!("Не найдена папка настроек"))?;
    crate::declarative::package::remove(&root, id)?;
    declarative::rescan(&builtin_ids());
    prefs::update(cx, |preferences| preferences.extensions.forget(id));
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

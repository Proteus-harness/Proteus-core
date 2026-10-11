//! Extensions installed as folders: an `extension.json` and a process that
//! draws its surfaces with widget trees (see [`crate::declarative`]).
//!
//! They join the catalog next to the built-in ones and reach the client only
//! through [`Host`], like any other extension.

mod badge;
mod render;
mod runtime;
mod view;

use std::collections::HashMap;
use std::sync::OnceLock;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::*;

use super::{BadgeSurface, DocumentSurface, Host, Manifest, PanelSurface, SurfaceView};
use crate::declarative::manifest::{self, Installed, Problem};
use crate::declarative::protocol::SurfaceKind;
use runtime::Runtime;
use view::DeclarativeView;

struct Loaded {
    installed: Vec<Installed>,
    problems: Vec<Problem>,
}

static LOADED: OnceLock<Loaded> = OnceLock::new();

/// Scans the extensions folder once; `taken` are ids of built-in extensions.
fn loaded(taken: &[&str]) -> &'static Loaded {
    LOADED.get_or_init(|| {
        let Some(root) = manifest::root() else {
            return Loaded {
                installed: Vec::new(),
                problems: Vec::new(),
            };
        };
        let (found, mut problems) = manifest::discover(&root);
        let mut installed = Vec::new();
        for extension in found {
            if taken.contains(&extension.manifest.id.as_str()) {
                problems.push(Problem {
                    dir: extension.dir,
                    error: format!(
                        "id `{}` занят встроенным расширением",
                        extension.manifest.id
                    ),
                });
            } else {
                installed.push(extension);
            }
        }
        Loaded {
            installed,
            problems,
        }
    })
}

/// Catalog entries of installed extensions.
pub(super) fn manifests(taken: &[&str]) -> Vec<Manifest> {
    loaded(taken)
        .installed
        .iter()
        .map(|extension| {
            let manifest = &extension.manifest;
            let surfaces = &manifest.surfaces;
            Manifest {
                id: &manifest.id,
                title: &manifest.title,
                description: &manifest.description,
                icon: icon(manifest.icon.as_deref()),
                panel: surfaces.panel.as_ref().map(|panel| PanelSurface {
                    title: &panel.title,
                    build: build_panel,
                }),
                document: surfaces.document.as_ref().map(|_| DocumentSurface {
                    build: build_document,
                    label: |argument| argument.to_string().into(),
                }),
                badge: surfaces.badge.as_ref().map(|badge| BadgeSurface {
                    build: badge::build,
                    place: badge.place,
                }),
            }
        })
        .collect()
}

/// Folders that look like extensions but did not load.
pub(super) fn problems() -> &'static [Problem] {
    LOADED.get().map_or(&[], |loaded| &loaded.problems)
}

fn installed(id: &str) -> Option<&'static Installed> {
    LOADED
        .get()?
        .installed
        .iter()
        .find(|extension| extension.manifest.id == id)
}

/// Icons an extension may name in its manifest.
fn icon(name: Option<&str>) -> Lucide {
    match name.unwrap_or("") {
        "activity" => Lucide::Activity,
        "bell" => Lucide::Bell,
        "book" => Lucide::BookOpen,
        "chart" => Lucide::ChartColumn,
        "clock" => Lucide::Clock,
        "code" => Lucide::Code,
        "database" => Lucide::Database,
        "flask" => Lucide::FlaskConical,
        "gauge" => Lucide::Gauge,
        "git" => Lucide::GitBranch,
        "globe" => Lucide::Globe,
        "list" => Lucide::ListChecks,
        "notes" => Lucide::NotebookPen,
        "server" => Lucide::Server,
        "terminal" => Lucide::Terminal,
        _ => Lucide::Puzzle,
    }
}

/// Every surface of an extension shares one process while any is shown.
#[derive(Default)]
struct Runtimes(HashMap<&'static str, WeakEntity<Runtime>>);

impl Global for Runtimes {}

fn runtime_of(id: &str, cx: &App) -> Option<Entity<Runtime>> {
    cx.try_global::<Runtimes>()?.0.get(id)?.upgrade()
}

fn runtime(host: &Host, window: &Window, cx: &mut App) -> Option<Entity<Runtime>> {
    let extension = installed(host.extension())?;
    if let Some(runtime) = runtime_of(&extension.manifest.id, cx) {
        return Some(runtime);
    }
    let runtime = cx.new(|cx| Runtime::new(extension, host.clone(), window, cx));
    cx.default_global::<Runtimes>()
        .0
        .insert(&extension.manifest.id, runtime.downgrade());
    Some(runtime)
}

fn surface(
    host: Host,
    kind: SurfaceKind,
    argument: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> SurfaceView {
    let runtime = runtime(&host, window, cx).expect("catalog lists installed extensions only");
    let view = cx.new(|cx| DeclarativeView::new(runtime, kind, argument, cx));
    let focus = view.read(cx).focus();
    SurfaceView {
        view: view.into(),
        focus,
    }
}

fn build_panel(host: Host, window: &mut Window, cx: &mut App) -> SurfaceView {
    surface(host, SurfaceKind::Panel, None, window, cx)
}

fn build_document(host: Host, argument: &str, window: &mut Window, cx: &mut App) -> SurfaceView {
    surface(
        host,
        SurfaceKind::Document,
        Some(argument.into()),
        window,
        cx,
    )
}

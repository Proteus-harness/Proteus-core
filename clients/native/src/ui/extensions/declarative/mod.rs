//! Extensions installed as folders: an `extension.json` and a process that
//! draws its surfaces with widget trees (see [`crate::declarative`]).
//!
//! They join the catalog next to the built-in ones, also right after an
//! install or removal, and reach the client only through [`Host`], like any
//! other extension.

mod badge;
mod render;
mod runtime;
mod view;

use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::*;

use super::{BadgeSurface, DocumentSurface, Host, Manifest, PanelSurface, SurfaceView};
use crate::declarative::manifest::{self, Installed, Problem};
use crate::declarative::protocol::SurfaceKind;
use runtime::Runtime;
use view::DeclarativeView;

/// An installed extension and its catalog entry. Both live for the rest of
/// the run, so open surfaces keep working after the folder is rescanned.
#[derive(Clone, Copy)]
struct Entry {
    installed: &'static Installed,
    manifest: &'static Manifest,
}

struct Loaded {
    entries: Vec<Entry>,
    problems: Vec<Problem>,
}

static LOADED: RwLock<Option<Loaded>> = RwLock::new(None);

/// Reads the extensions folder on first use; `taken` are built-in ids.
fn with_loaded<T>(taken: &[&str], read: impl FnOnce(&Loaded) -> T) -> T {
    if let Some(loaded) = LOADED.read().unwrap().as_ref() {
        return read(loaded);
    }
    rescan(taken);
    read(LOADED.read().unwrap().as_ref().expect("just scanned"))
}

/// Reads the extensions folder again. Entries that did not change are kept,
/// so their surfaces and processes stay as they are.
pub(super) fn rescan(taken: &[&str]) {
    let mut loaded = LOADED.write().unwrap();
    let previous = loaded.take().map_or_else(Vec::new, |loaded| loaded.entries);
    let (found, mut problems) = manifest::root()
        .map(|root| manifest::discover(&root))
        .unwrap_or_default();
    let mut entries = Vec::new();
    for extension in found {
        if taken.contains(&extension.manifest.id.as_str()) {
            problems.push(Problem {
                dir: extension.dir,
                error: format!(
                    "id `{}` занят встроенным расширением",
                    extension.manifest.id
                ),
            });
            continue;
        }
        let kept = previous
            .iter()
            .find(|entry| *entry.installed == extension)
            .copied();
        entries.push(kept.unwrap_or_else(|| entry(extension)));
    }
    *loaded = Some(Loaded { entries, problems });
}

fn entry(extension: Installed) -> Entry {
    let installed: &'static Installed = Box::leak(Box::new(extension));
    let manifest = &installed.manifest;
    let surfaces = &manifest.surfaces;
    let manifest = Box::leak(Box::new(Manifest {
        id: &manifest.id,
        title: &manifest.title,
        description: &manifest.description,
        icon: icon(manifest.icon.as_deref()),
        page: None,
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
            // The face carries text of any length.
            narrow: false,
        }),
    }));
    Entry {
        installed,
        manifest,
    }
}

/// Catalog entries of installed extensions.
pub(super) fn manifests(taken: &[&str]) -> Vec<&'static Manifest> {
    with_loaded(taken, |loaded| {
        loaded.entries.iter().map(|entry| entry.manifest).collect()
    })
}

/// Folders that look like extensions but did not load.
pub(super) fn problems(taken: &[&str]) -> Vec<Problem> {
    with_loaded(taken, |loaded| loaded.problems.clone())
}

pub(super) fn folder(taken: &[&str], id: &str) -> Option<&'static Path> {
    with_loaded(taken, |loaded| {
        loaded
            .entries
            .iter()
            .find(|entry| entry.manifest.id == id)
            .map(|entry| entry.installed.dir.as_path())
    })
}

fn installed(id: &str) -> Option<&'static Installed> {
    LOADED
        .read()
        .unwrap()
        .as_ref()?
        .entries
        .iter()
        .find(|entry| entry.manifest.id == id)
        .map(|entry| entry.installed)
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

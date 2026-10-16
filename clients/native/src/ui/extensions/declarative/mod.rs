//! Extensions installed as folders: an `extension.json` and a process that
//! draws its surfaces with widget trees (see [`crate::declarative`]).
//!
//! Shipped and user-installed packages share this renderer and [`Host`].

mod badge;
mod fields;
mod graph;
mod popup;
mod render;
mod runtime;
mod steps;
mod terminal;
mod tree;
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

/// Reads shipped and user-installed packages on first use.
fn with_loaded<T>(read: impl FnOnce(&Loaded) -> T) -> T {
    if let Some(loaded) = LOADED.read().unwrap().as_ref() {
        return read(loaded);
    }
    rescan();
    read(LOADED.read().unwrap().as_ref().expect("just scanned"))
}

/// Reads the extensions folder again. Entries that did not change are kept,
/// so their surfaces and processes stay as they are.
pub(super) fn rescan() {
    let mut loaded = LOADED.write().unwrap();
    let previous = loaded.take().map_or_else(Vec::new, |loaded| loaded.entries);
    let (found, problems) =
        manifest::discover_sources(&manifest::bundled_root(), manifest::root().as_deref());
    let mut entries = Vec::new();
    for extension in found {
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
        author: &manifest.author,
        description: &manifest.description,
        icon: icon(manifest.icon.as_deref()),
        page: surfaces
            .page
            .as_ref()
            .map(|_| build_page as super::BuildFocused),
        panel: surfaces.panel.as_ref().map(|panel| PanelSurface {
            title: &panel.title,
            build: build_panel,
        }),
        document: surfaces.document.as_ref().map(|document| DocumentSurface {
            build: build_document,
            label: document.label.as_deref(),
            command: document.command.as_deref(),
        }),
        badge: surfaces.badge.as_ref().map(|badge| BadgeSurface {
            build: badge::build,
            place: badge.place,
            narrow: badge.narrow,
        }),
    }));
    Entry {
        installed,
        manifest,
    }
}

/// Catalog entries of installed extensions.
pub(super) fn manifests() -> Vec<&'static Manifest> {
    with_loaded(|loaded| loaded.entries.iter().map(|entry| entry.manifest).collect())
}

/// Folders that look like extensions but did not load.
pub(super) fn problems() -> Vec<Problem> {
    with_loaded(|loaded| loaded.problems.clone())
}

pub(super) fn folder(id: &str) -> Option<&'static Path> {
    with_loaded(|loaded| {
        loaded
            .entries
            .iter()
            .find(|entry| entry.manifest.id == id)
            .filter(|entry| manifest::root().as_deref() == entry.installed.dir.parent())
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
        "network" => Lucide::Network,
        "folder" => Lucide::FolderTree,
        "shield" => Lucide::ShieldCheck,
        "cpu" => Lucide::Cpu,
        "layers" => Lucide::Layers,
        "info" => Lucide::Info,
        "bot" => Lucide::Bot,
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

/// Every surface shares one process, retained until its workspace closes.
#[derive(Default)]
struct Runtimes(HashMap<(EntityId, &'static str), WeakEntity<Runtime>>);

impl Global for Runtimes {}

fn runtime_of(host: &Host, cx: &App) -> Option<Entity<Runtime>> {
    cx.try_global::<Runtimes>()?
        .0
        .get(&(host.identity(), host.extension()))?
        .upgrade()
}

fn runtime(host: &Host, window: &Window, cx: &mut App) -> Option<Entity<Runtime>> {
    let extension = installed(host.extension())?;
    if let Some(runtime) = runtime_of(host, cx) {
        return Some(runtime);
    }
    let runtime = cx.new(|cx| Runtime::new(extension, host.clone(), window, cx));
    cx.default_global::<Runtimes>().0.insert(
        (host.identity(), &extension.manifest.id),
        runtime.downgrade(),
    );
    let (owner, process) = (host.clone(), runtime.clone());
    cx.defer(move |cx| owner.retain_process(process.into(), cx));
    Some(runtime)
}

pub(super) fn surface(
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

fn build_page(host: Host, window: &mut Window, cx: &mut App) -> SurfaceView {
    surface(host, SurfaceKind::Page, None, window, cx)
}

/// Reload packages in live surfaces after an explicit install/update/rescan.
pub(super) fn reload_runtimes(cx: &mut App) {
    let runtimes: Vec<_> = cx
        .try_global::<Runtimes>()
        .map(|runtimes| {
            runtimes
                .0
                .values()
                .filter_map(WeakEntity::upgrade)
                .collect()
        })
        .unwrap_or_default();
    for runtime in runtimes {
        runtime.update(cx, |runtime, cx| runtime.reload(cx));
    }
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

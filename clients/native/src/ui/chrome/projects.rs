//! Project items of the top bar menus: recent folders and the folder
//! dialog, routed to whoever owns the window content.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::Icon;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;

use crate::ui::prefs;

/// Recent folders listed in a menu; the picker screen shows all of them.
const RECENT_SHOWN: usize = 8;

/// The workspace asks the window to show another folder in its place.
pub(in crate::ui) enum WorkspaceRequest {
    Open(PathBuf),
    Choose,
}

type OpenFolder = Rc<dyn Fn(PathBuf, &mut Window, &mut App)>;
type ChooseFolder = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub(in crate::ui) struct Projects {
    current: Option<PathBuf>,
    open: OpenFolder,
    choose: ChooseFolder,
}

impl Projects {
    pub(in crate::ui) fn new(
        current: Option<PathBuf>,
        open: impl Fn(PathBuf, &mut Window, &mut App) + 'static,
        choose: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            current,
            open: Rc::new(open),
            choose: Rc::new(choose),
        }
    }

    /// Recent folders other than the open one, then the folder dialog.
    pub(in crate::ui) fn items(&self, mut menu: PopupMenu, cx: &App) -> PopupMenu {
        let recent: Vec<PathBuf> = prefs::current(cx)
            .recent
            .iter()
            .filter(|path| Some(*path) != self.current.as_deref())
            .take(RECENT_SHOWN)
            .map(Path::to_path_buf)
            .collect();
        if !recent.is_empty() {
            menu = menu.label("Недавние проекты");
        }
        for path in recent {
            let open = self.open.clone();
            menu = menu.item(
                PopupMenuItem::new(folder_name(&path))
                    .icon(Icon::new(Lucide::Folder))
                    .disabled(!path.is_dir())
                    .on_click(move |_, window, cx| open(path.clone(), window, cx)),
            );
        }
        let choose = self.choose.clone();
        menu.item(
            PopupMenuItem::new("Открыть папку…")
                .icon(Icon::new(Lucide::FolderOpen))
                .on_click(move |_, window, cx| choose(window, cx)),
        )
    }
}

pub(in crate::ui) fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

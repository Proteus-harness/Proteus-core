mod changes;
mod document;
mod tree;

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{Host, SurfaceView};
use crate::files::ChangeStatus;
use crate::ui::fade::faded;

pub(super) use tree::bind_keys;

pub(super) const ID: &str = "files";

pub(super) fn build_panel(host: Host, window: &mut Window, cx: &mut App) -> SurfaceView {
    let view = cx.new(|cx| tree::FilesPanel::new(host, window, cx));
    SurfaceView {
        focus: view.read(cx).focus_handle.clone(),
        view: view.into(),
    }
}

/// File tab for `path`, relative to the session working directory.
pub(super) fn build_document(
    host: Host,
    path: &str,
    window: &mut Window,
    cx: &mut App,
) -> SurfaceView {
    let view = cx.new(|cx| document::FileDocument::new(host, path.to_string(), window, cx));
    SurfaceView {
        focus: view.read(cx).focus_handle(cx),
        view: view.into(),
    }
}

/// Tab title: the file name.
pub(super) fn document_label(path: &str) -> SharedString {
    crate::files::split_path(path).1.to_string().into()
}

pub(super) fn build_badge(host: Host, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| changes::FilesBadge::new(host, window, cx))
        .into()
}

fn status_color(status: ChangeStatus, cx: &App) -> Hsla {
    let theme = cx.theme();
    match status {
        ChangeStatus::Added | ChangeStatus::Untracked => theme.success,
        ChangeStatus::Modified | ChangeStatus::Renamed => theme.warning,
        ChangeStatus::Deleted | ChangeStatus::Conflict => theme.danger,
    }
}

/// `dir/` in muted text followed by the file name.
fn path_label(path: &str, cx: &App) -> Div {
    let (dir, name) = crate::files::split_path(path);
    h_flex()
        .min_w_0()
        .overflow_hidden()
        .child(div().flex_none().child(name.to_string()))
        .when(!dir.is_empty(), |label| {
            label.child(
                div()
                    .min_w_0()
                    .pl_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(faded(dir.to_string())),
            )
        })
}

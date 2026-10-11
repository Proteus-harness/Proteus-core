mod changes;
mod page;

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{Host, PageView};
use crate::files::ChangeStatus;

pub(super) use page::{FilesPage, Mode, bind_keys};

pub(super) const ID: &str = "files";

pub(super) fn build_page(host: Host, window: &mut Window, cx: &mut App) -> PageView {
    let page = cx.new(|cx| FilesPage::new(host, window, cx));
    PageView {
        focus: page.read(cx).focus_handle.clone(),
        view: page.into(),
    }
}

pub(super) fn build_changes(host: Host, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| changes::ChangesWidget::new(host, window, cx))
        .into()
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
                    .truncate()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(dir.to_string()),
            )
        })
}

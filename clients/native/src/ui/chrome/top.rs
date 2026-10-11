//! The bar above the tabs: the logo menu, the open folder and extension
//! icons. It also moves the window.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::logo::logo;
use super::projects::{Projects, WorkspaceRequest, folder_name};
use crate::preferences::BadgePlace;
use crate::ui::chat::ChatState;
use crate::ui::prefs;
use crate::ui::tip::Tip as _;
use crate::ui::workspace::{Status, Workspace};

/// Logo cell size before zoom.
const LOGO_CELL: f32 = 2.;

impl Workspace {
    pub(in crate::ui) fn render_top_bar(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let workspace = cx.weak_entity();
        let projects = self.switchable.then(|| {
            let (open, choose) = (workspace.clone(), workspace.clone());
            Projects::new(
                Some(self.workspace_path.clone()),
                move |path, _, cx| {
                    open.update(cx, |_, cx| cx.emit(WorkspaceRequest::Open(path)))
                        .ok();
                },
                move |_, cx| {
                    choose
                        .update(cx, |_, cx| cx.emit(WorkspaceRequest::Choose))
                        .ok();
                },
            )
        });
        let ready = matches!(self.status, Status::Ready);
        let busy = self.chats.values().any(|chat| {
            matches!(chat.view.read(cx).session.state(), ChatState::Busy(_))
        });
        let folder = folder_name(&self.workspace_path);
        let place: SharedString = self.workspace_path.display().to_string().into();
        let folder = match projects.clone() {
            Some(projects) => Button::new("top-folder")
                .ghost()
                .small()
                .label(folder)
                .tooltip(place)
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, cx| {
                    projects.items(menu, cx)
                })
                .into_any_element(),
            None => div()
                .id("top-folder")
                .px_2()
                .text_sm()
                .child(folder)
                .tip_text(place)
                .into_any_element(),
        };
        let left = h_flex()
            .gap_1()
            .items_center()
            .child(logo_button(busy, logo_menu(projects)))
            .child(folder);
        let right = h_flex()
            .gap_1()
            .items_center()
            .when(ready, |right| {
                right.child(self.render_badge_strip(BadgePlace::Header, cx))
            });
        bar(left, right, theme)
    }
}

/// The bar of the folder picker: only the logo menu.
pub(in crate::ui) fn picker_top_bar(projects: Projects, cx: &App) -> impl IntoElement + use<> {
    bar(
        logo_button(false, logo_menu(Some(projects))),
        div(),
        cx.theme(),
    )
}

fn bar(left: impl IntoElement, right: impl IntoElement, theme: &Theme) -> TitleBar {
    TitleBar::new()
        .h(prefs::scaled(super::BAR_HEIGHT))
        .pl_1()
        .pr_1()
        .bg(theme.title_bar)
        .border_color(theme.title_bar_border)
        .child(left)
        .child(right)
}

fn logo_button(
    busy: bool,
    menu: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
) -> impl IntoElement {
    Button::new("top-logo")
        .ghost()
        .small()
        .tooltip(if busy { "Proteus · агент работает" } else { "Proteus" })
        .child(logo(LOGO_CELL, busy))
        .dropdown_menu_with_anchor(Anchor::TopLeft, menu)
}

/// Projects, the version and quitting; settings have their own button.
fn logo_menu(
    projects: Option<Projects>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |mut menu, _, cx| {
        if let Some(projects) = &projects {
            menu = projects.items(menu, cx).separator();
        }
        menu.label(concat!("Proteus native ", env!("CARGO_PKG_VERSION")))
            .item(
                PopupMenuItem::new("Выход")
                    .icon(Icon::new(Lucide::LogOut))
                    .on_click(|_, _, cx| cx.quit()),
            )
    }
}

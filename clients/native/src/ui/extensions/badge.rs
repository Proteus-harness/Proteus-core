//! Compact extension icons, shown under the composer or in the tab bar. A
//! click brings the extension forward, the context menu moves or hides the
//! icon, and icons can be dragged between both places.

use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::*;

use super::frame::drag_preview;
use super::host::Host;
use crate::preferences::BadgePlace;
use crate::ui::prefs;

/// Payload of an icon being dragged; also its drag preview.
#[derive(Clone)]
pub(in crate::ui) struct DraggedBadge {
    pub id: &'static str,
    pub title: SharedString,
}

impl Render for DraggedBadge {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drag_preview(self.title.clone(), cx)
    }
}

/// Tooltip lines of an icon: a title and optional details.
pub(super) struct Details {
    pub title: SharedString,
    pub lines: Vec<SharedString>,
}

impl Details {
    pub(super) fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            lines: Vec::new(),
        }
    }

    pub(super) fn line(mut self, line: impl Into<SharedString>) -> Self {
        self.lines.push(line.into());
        self
    }
}

/// The shared shell of every icon: hover, tooltip, click, drag and menu.
pub(super) fn badge_frame<F: IntoElement + 'static>(
    id: &'static str,
    face: F,
    details: Details,
    host: &Host,
    cx: &App,
) -> impl IntoElement + use<F> {
    let theme = cx.theme();
    let (hover, foreground) = (theme.list_hover, theme.foreground);
    let title = super::find(id).map_or(id, |manifest| manifest.title);
    let (click_host, menu_host) = (host.clone(), host.clone());
    let details = std::rc::Rc::new(details);
    div()
        .id(ElementId::Name(format!("badge-{id}").into()))
        .h_7()
        .min_w_7()
        .px_1()
        .flex()
        .items_center()
        .justify_center()
        .gap_1()
        .rounded(theme.radius)
        .text_xs()
        .text_color(theme.muted_foreground)
        .cursor_pointer()
        .hover(move |style| style.bg(hover).text_color(foreground))
        .child(face)
        .tooltip(move |window, cx| {
            let details = details.clone();
            Tooltip::element(move |_, cx| tooltip_body(&details, cx)).build(window, cx)
        })
        .on_click(move |_, window, cx| click_host.activate(id, window, cx))
        .on_drag(
            DraggedBadge {
                id,
                title: title.into(),
            },
            |dragged, _, _, cx| cx.new(|_| dragged.clone()),
        )
        .context_menu(move |menu, _, cx| badge_menu(menu, id, &menu_host, cx))
}

fn tooltip_body(details: &Details, cx: &App) -> Div {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .max_w(px(320.))
        .gap_0p5()
        .child(div().text_sm().font_semibold().child(details.title.clone()))
        .children(
            details
                .lines
                .iter()
                .map(|line| div().text_xs().text_color(muted).child(line.clone())),
        )
}

fn badge_menu(menu: PopupMenu, id: &'static str, host: &Host, cx: &App) -> PopupMenu {
    let Some(manifest) = super::find(id) else {
        return menu;
    };
    let mut menu = menu;
    if manifest.page.is_some() {
        let host = host.clone();
        menu = menu.item(
            PopupMenuItem::new("Открыть вкладку")
                .on_click(move |_, window, cx| host.activate(id, window, cx)),
        );
    }
    if manifest.panel.is_some() {
        let host = host.clone();
        menu = menu.item(
            PopupMenuItem::new("Показать в панели")
                .on_click(move |_, window, cx| host.show_widget(id, window, cx)),
        );
    }
    let default = manifest
        .badge
        .as_ref()
        .map_or(BadgePlace::Hidden, |badge| badge.place);
    let current = prefs::current(cx).extensions.badge_place(id, default);
    menu = menu.separator();
    for (place, label) in [
        (BadgePlace::Composer, "Под полем ввода"),
        (BadgePlace::Header, "В панели вкладок"),
        (BadgePlace::Hidden, "Скрыть значок"),
    ] {
        menu = menu.item(
            PopupMenuItem::new(label)
                .checked(current == place)
                .on_click(move |_, _, cx| {
                    prefs::update(cx, |prefs| prefs.extensions.set_badge_place(id, place))
                }),
        );
    }
    menu
}

/// Icon of an extension with a small counter next to it.
pub(super) fn icon_with_count(icon: Icon, count: Option<String>) -> Div {
    h_flex()
        .gap_1()
        .child(icon.small())
        .children(count.map(|count| div().text_xs().child(count)))
}

//! Compact extension icons, shown under the composer or in the tab bar.
//! Details live in the hover tooltip; a click brings the extension forward
//! when it has a view; the context menu moves, hides or pins the icon, and
//! unpinned icons can be dragged between both places.

use std::rc::Rc;

use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::host::Host;
use crate::preferences::BadgePlace;
use crate::ui::{drag, prefs};

/// What an icon shows; drawn again for its drag preview.
pub(super) type Face = Rc<dyn Fn() -> AnyElement>;

/// Payload of an icon being dragged; renders as a lifted copy of the icon.
#[derive(Clone)]
pub(in crate::ui) struct DraggedBadge {
    pub id: &'static str,
    face: Face,
}

impl Render for DraggedBadge {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let foreground = cx.theme().foreground;
        drag::lifted(badge_box().text_color(foreground).child((self.face)()), cx)
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

fn badge_box() -> Div {
    div()
        .h_7()
        .min_w_7()
        .px_1()
        .flex()
        .items_center()
        .justify_center()
        .gap_1()
        .text_xs()
}

/// The shared shell of every icon: hover, tooltip, click, drag and menu.
pub(super) fn badge_frame(
    id: &'static str,
    face: Face,
    details: Details,
    host: &Host,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (hover, foreground) = (theme.list_hover, theme.foreground);
    let opens = super::find(id).is_some_and(|manifest| manifest.sidebar.is_some());
    let pinned = prefs::current(cx).extensions.is_pinned(id);
    let lifted = drag::is_source(&drag_key(id), cx);
    let (click_host, menu_host) = (host.clone(), host.clone());
    let details = Rc::new(details);
    let dragged = DraggedBadge {
        id,
        face: face.clone(),
    };
    badge_box()
        .id(ElementId::Name(format!("badge-{id}").into()))
        .rounded(theme.radius)
        .text_color(theme.muted_foreground)
        .when(opens, |badge| {
            badge
                .cursor_pointer()
                .on_click(move |_, window, cx| click_host.activate(id, window, cx))
        })
        .hover(move |style| style.bg(hover).text_color(foreground))
        .when(lifted, |badge| badge.opacity(drag::SOURCE_OPACITY))
        .child(face())
        .tooltip(move |window, cx| {
            let details = details.clone();
            Tooltip::element(move |_, cx| tooltip_body(&details, cx)).build(window, cx)
        })
        .when(!pinned, |badge| {
            badge.on_drag(dragged, |dragged, _, _, cx| {
                drag::start(drag_key(dragged.id), cx);
                cx.new(|_| dragged.clone())
            })
        })
        .context_menu(move |menu, _, cx| badge_menu(menu, id, &menu_host, cx))
}

fn drag_key(id: &str) -> String {
    format!("badge:{id}")
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
    if let Some(sidebar) = &manifest.sidebar {
        let host = host.clone();
        menu = menu
            .item(
                PopupMenuItem::new(format!("Открыть «{}» слева", sidebar.title))
                    .on_click(move |_, window, cx| host.activate(id, window, cx)),
            )
            .separator();
    }
    let default = manifest
        .badge
        .as_ref()
        .map_or(BadgePlace::Hidden, |badge| badge.place);
    let extensions = &prefs::current(cx).extensions;
    let current = extensions.badge_place(id, default);
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
    let pinned = extensions.is_pinned(id);
    menu.separator().item(
        PopupMenuItem::new("Закрепить на месте")
            .checked(pinned)
            .on_click(move |_, _, cx| {
                prefs::update(cx, |prefs| prefs.extensions.set_pinned(id, !pinned))
            }),
    )
}

/// Icon of an extension with a small counter next to it.
pub(super) fn icon_with_count(icon: Icon, count: Option<String>) -> Div {
    h_flex()
        .gap_1()
        .child(icon.small())
        .children(count.map(|count| div().text_xs().child(count)))
}

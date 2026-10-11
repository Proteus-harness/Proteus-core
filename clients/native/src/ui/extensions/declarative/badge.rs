//! Compact icon of an installed extension: its icon or the short text the
//! process puts at the top of the badge tree.

use std::rc::Rc;

use gpui_kit::component::*;
use gpui_kit::*;

use super::super::badge::{Details, badge_frame};
use super::view::DeclarativeView;
use super::{Host, SurfaceKind, installed, runtime};

struct DeclarativeBadge {
    host: Host,
    view: Entity<DeclarativeView>,
    _view: Subscription,
}

pub(super) fn build(host: Host, window: &mut Window, cx: &mut App) -> AnyView {
    let runtime = runtime(&host, window, cx).expect("catalog lists installed extensions only");
    let view = cx.new(|cx| DeclarativeView::new(runtime, SurfaceKind::Badge, None, cx));
    cx.new(|cx| DeclarativeBadge {
        _view: cx.observe(&view, |_, _, cx| cx.notify()),
        host,
        view,
    })
    .into()
}

impl Render for DeclarativeBadge {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = self.host.extension();
        let Some(extension) = installed(id) else {
            return div().into_any_element();
        };
        let manifest = &extension.manifest;
        let icon = super::icon(manifest.icon.as_deref());
        let view = self.view.read(cx);
        let mut details = Details::new(manifest.title.clone());
        if !manifest.description.is_empty() {
            details = details.line(manifest.description.clone());
        }
        if let Some(error) = view.error(cx) {
            details = details.line(format!("Ошибка: {error}"));
        }
        let face: Rc<dyn Fn() -> AnyElement> = match view.face(cx) {
            Some((text, color)) => Rc::new(move || {
                h_flex()
                    .gap_1()
                    .child(Icon::new(icon).small())
                    .child(div().text_color(color).child(text.clone()))
                    .into_any_element()
            }),
            None => Rc::new(move || Icon::new(icon).small().into_any_element()),
        };
        badge_frame(id, face, details, &self.host, cx).into_any_element()
    }
}

//! Compact icon of an installed extension: its icon or the short text the
//! process puts at the top of the badge tree.

use std::rc::Rc;

use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::*;
use gpui_kit::*;

use super::super::badge::{Details, Press, pressable_badge};
use super::view::DeclarativeView;
use super::{Host, SurfaceKind, installed, runtime};
use crate::declarative::presentation::Face;
use crate::ui::{motion::Number, prefs};
use std::cell::Cell;

pub(super) struct DeclarativeBadge {
    pub(super) host: Host,
    pub(super) view: Entity<DeclarativeView>,
    pub(super) popup: Option<super::super::SurfaceView>,
    pub(super) popup_epoch: u64,
    pub(super) bounds: Rc<Cell<Bounds<Pixels>>>,
    pub(super) restore: Option<FocusHandle>,
    _view: Subscription,
}

pub(super) fn build(host: Host, window: &mut Window, cx: &mut App) -> AnyView {
    let runtime = runtime(&host, window, cx).expect("catalog lists installed extensions only");
    let view = cx.new(|cx| DeclarativeView::new(runtime, SurfaceKind::Badge, None, cx));
    cx.new(|cx| DeclarativeBadge {
        _view: cx.observe(&view, |_, _, cx| cx.notify()),
        host,
        view,
        popup: None,
        popup_epoch: 0,
        bounds: Rc::default(),
        restore: None,
    })
    .into()
}

impl Render for DeclarativeBadge {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = self.host.extension();
        let Some(extension) = installed(id) else {
            return div().into_any_element();
        };
        let manifest = &extension.manifest;
        let icon = super::icon(manifest.icon.as_deref());
        let view = self.view.read(cx);
        if view
            .presentation
            .as_ref()
            .is_some_and(|presentation| presentation.hidden)
        {
            self.popup = None;
            return div().into_any_element();
        }
        let epoch = view.popup_closed(cx);
        if epoch != self.popup_epoch {
            self.popup_epoch = epoch;
            self.close_popup(window, cx);
        }
        let view = self.view.read(cx);
        let mut details = Details::new(manifest.title.clone());
        if !manifest.description.is_empty() {
            details = details.line(manifest.description.clone());
        }
        let presentation = view.presentation.as_ref();
        if let Some(presentation) = presentation {
            details = Details::new(presentation.title.clone());
            for line in &presentation.lines {
                details = details.line(line.clone());
            }
        }
        if let Some(error) = view.error(cx) {
            details = details.line(format!("Ошибка: {error}"));
        }
        let face: Rc<dyn Fn() -> AnyElement> = if let Some(presentation) = presentation {
            match &presentation.face {
                Face::Icon { tone, count } => {
                    let color = super::render::tone_color(*tone, cx);
                    let count = *count;
                    Rc::new(move || {
                        div()
                            .relative()
                            .size(prefs::scaled(22.))
                            .child(Icon::new(icon).small().text_color(color))
                            .children(count.filter(|count| *count > 0).map(|count| {
                                div()
                                    .absolute()
                                    .right_0()
                                    .bottom_0()
                                    .text_size(px(9.))
                                    .font_semibold()
                                    .child(if count > 99 {
                                        "99+".to_string()
                                    } else {
                                        count.to_string()
                                    })
                            }))
                            .into_any_element()
                    })
                }
                Face::Text {
                    text,
                    tone,
                    icon: show_icon,
                    caret,
                } => {
                    let (show_icon, caret) = (*show_icon, *caret);
                    let (text, color) = (text.clone(), super::render::tone_color(*tone, cx));
                    Rc::new(move || {
                        h_flex()
                            .gap_1()
                            .text_color(color)
                            .children(show_icon.then(|| Icon::new(icon).small()))
                            .child(text.clone())
                            .children(caret.then(|| {
                                Icon::new(gpui_kit::assets::IconName::ChevronDown).xsmall()
                            }))
                            .into_any_element()
                    })
                }
                Face::Ring { percent, tone } => {
                    let (percent, color) = (*percent, super::render::tone_color(*tone, cx));
                    Rc::new(move || {
                        ProgressCircle::new(id)
                            .value(percent)
                            .color(color)
                            .size(prefs::scaled(22.))
                            .child(div().text_size(px(9.)).font_semibold().child(Number::new(
                                ElementId::Name(format!("extension-{id}-number").into()),
                                percent,
                                |value| format!("{value:.0}"),
                            )))
                            .into_any_element()
                    })
                }
            }
        } else {
            match view.face(cx) {
                Some((text, color)) => Rc::new(move || {
                    h_flex()
                        .gap_1()
                        .child(Icon::new(icon).small())
                        .child(div().text_color(color).child(text.clone()))
                        .into_any_element()
                }),
                None => Rc::new(move || Icon::new(icon).small().into_any_element()),
            }
        };
        let press = manifest.surfaces.popup.as_ref().map(|_| {
            let this = cx.entity().downgrade();
            Rc::new(move |window: &mut Window, cx: &mut App| {
                let _ = this.update(cx, |this, cx| this.toggle_popup(window, cx));
            }) as Press
        });
        let bounds = self.bounds.clone();
        let popup = manifest
            .surfaces
            .popup
            .as_ref()
            .and_then(|spec| self.render_popup(spec.width, window, cx));
        div()
            .relative()
            .on_prepaint(move |rect, _, _| bounds.set(rect))
            .child(pressable_badge(id, face, details, &self.host, press, cx))
            .children(popup)
            .into_any_element()
    }
}

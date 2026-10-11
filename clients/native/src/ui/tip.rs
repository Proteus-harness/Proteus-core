//! Hover tooltips anchored to the hovered element. GPUI's own `.tooltip()`
//! opens next to the pointer; gpui-kit components already anchor theirs to the
//! trigger through a window overlay, and this module gives plain elements the
//! same behaviour with one overlay of our own.

use std::{cell::Cell, rc::Rc, time::Duration};

use gpui_kit::base::animation::{EffectTransition, ease_in_out_cubic, ease_out_cubic};
use gpui_kit::base::{ElementExt as _, TooltipOverlay, TooltipRequest, TooltipTransition};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Bounds, ElementId, Entity, Global, IntoElement,
    ParentElement as _, Pixels, SharedString, StatefulInteractiveElement, Window,
    div, px,
};

use super::motion;

const ENTER: Duration = Duration::from_millis(150);
const SLIDE: Duration = Duration::from_millis(200);

struct Overlay(Entity<TooltipOverlay>);

thread_local! {
    /// The element pressed last; its tooltip stays away until the pointer leaves.
    static PRESSED: Cell<Option<Bounds<Pixels>>> = const { Cell::new(None) };
}

impl Global for Overlay {}

/// The overlay that draws the open tooltip; render it once at the window root.
pub(super) fn layer(cx: &mut App) -> Entity<TooltipOverlay> {
    if let Some(overlay) = cx.try_global::<Overlay>() {
        return overlay.0.clone();
    }
    let overlay = cx.new(|_| TooltipOverlay::new().render_with(present));
    cx.set_global(Overlay(overlay.clone()));
    overlay
}

fn present(view: AnyView, transition: TooltipTransition, _: &mut Window, _: &mut App) -> AnyElement {
    let element = div().child(view);
    if !motion::plays() {
        return element.into_any_element();
    }
    match transition {
        TooltipTransition::Switch {
            epoch,
            previous,
            current,
        } => {
            if (current.origin.y - previous.origin.y).abs() >= px(10.) {
                return element.into_any_element();
            }
            let shift = current.center().x - previous.center().x;
            EffectTransition::new(SLIDE)
                .ease(ease_in_out_cubic)
                .slide_x(-shift, px(0.))
                .apply(element, ElementId::NamedInteger("tip-slide".into(), epoch as u64))
                .into_any_element()
        }
        TooltipTransition::Enter { epoch } => EffectTransition::new(ENTER)
            .ease(ease_out_cubic)
            .slide_y(px(4.), px(0.))
            .fade(0., 1.)
            .apply(element, ElementId::NamedInteger("tip-enter".into(), epoch as u64))
            .into_any_element(),
    }
}

fn update(cx: &mut App, window: &mut Window, f: impl FnOnce(&mut TooltipOverlay, &mut Window, &mut gpui_kit::Context<TooltipOverlay>)) {
    if let Some(overlay) = cx.try_global::<Overlay>().map(|overlay| overlay.0.clone()) {
        overlay.update(cx, |overlay, cx| f(overlay, window, cx));
    }
}

pub(crate) trait Tip: StatefulInteractiveElement + gpui_kit::ParentElement + Sized {
    /// Shows `build` beside this element while the pointer rests on it. A
    /// press hides it until the pointer leaves, so it never covers the menu
    /// or drag the press starts.
    fn tip(self, build: impl Fn(&mut Window, &mut App) -> AnyView + 'static) -> Self {
        let build = Rc::new(build);
        let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::default();
        let (measured, pressed) = (bounds.clone(), bounds.clone());
        self.on_prepaint(move |rect, _, _| measured.set(rect))
            .on_hover(move |hovered, window, cx| {
                let hovered = *hovered;
                let build = build.clone();
                let trigger = bounds.get();
                if !hovered {
                    PRESSED.set(None);
                } else if PRESSED.get() == Some(trigger) {
                    return;
                }
                update(cx, window, move |overlay, window, cx| {
                    if hovered {
                        let request = TooltipRequest::new(trigger, move |window, cx| build(window, cx));
                        overlay.request_show(request, window, cx);
                    } else {
                        overlay.request_hide(window, cx);
                    }
                });
            })
            // Capture: the element's own controls may stop the press.
            .capture_any_mouse_down(move |_, window, cx| {
                PRESSED.set(Some(pressed.get()));
                update(cx, window, |overlay, _, cx| overlay.hide(cx));
            })
    }

    /// A plain text tooltip.
    fn tip_text(self, text: impl Into<SharedString>) -> Self {
        let text = text.into();
        self.tip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
    }

    /// A tooltip built from an element on every show.
    fn tip_element<E: IntoElement>(self, body: impl Fn(&mut Window, &mut App) -> E + 'static) -> Self {
        let body = Rc::new(body);
        self.tip(move |window, cx| {
            let body = body.clone();
            Tooltip::element(move |window, cx| body(window, cx)).build(window, cx)
        })
    }
}

impl<E: StatefulInteractiveElement + gpui_kit::ParentElement> Tip for E {}

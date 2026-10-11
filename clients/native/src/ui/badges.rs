use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::extensions::{self, DraggedBadge, Host};
use super::workspace::Workspace;
use super::{drag, prefs};
use crate::preferences::BadgePlace;

impl Workspace {
    /// Builds icons that became visible and drops the hidden ones, so an
    /// icon keeps its state while it moves between the strips.
    pub(super) fn sync_badges(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible: Vec<_> = [BadgePlace::Composer, BadgePlace::Header, BadgePlace::Status]
            .into_iter()
            .flat_map(|place| extensions::visible_badges(place, cx))
            .collect();
        self.badges
            .retain(|id, _| visible.iter().any(|manifest| manifest.id == *id));
        for manifest in visible {
            let Some(badge) = &manifest.badge else {
                continue;
            };
            if !self.badges.contains_key(manifest.id) {
                let view = (badge.build)(Host::new(cx.weak_entity(), manifest.id), window, cx);
                self.badges.insert(manifest.id, view);
            }
        }
    }

    /// Row of extension icons; icons are dropped here to move or reorder them.
    pub(super) fn render_badge_strip(
        &self,
        place: BadgePlace,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let dragging = drag::is_dragging("badge:", cx);
        let manifests = extensions::visible_badges(place, cx);
        let empty = manifests.is_empty();
        let slots: Vec<_> = manifests
            .into_iter()
            .filter_map(|manifest| {
                let view = self.badges.get(manifest.id)?.clone();
                Some(
                    drop_slot(slot_name(place, manifest.id), place, Some(manifest.id), cx)
                        .child(view),
                )
            })
            .collect();
        h_flex()
            .flex_none()
            .items_center()
            .gap_0p5()
            .children(slots)
            .when(dragging || empty, |strip| {
                strip.child(
                    drop_slot(slot_name(place, "end"), place, None, cx)
                        .h_7()
                        .when(dragging, |slot| slot.min_w(prefs::scaled(28.))),
                )
            })
    }
}

fn slot_name(place: BadgePlace, key: &str) -> SharedString {
    let place = match place {
        BadgePlace::Composer => "composer",
        BadgePlace::Header => "header",
        BadgePlace::Status => "status",
        BadgePlace::Hidden => "hidden",
    };
    SharedString::from(format!("badge-slot-{place}-{key}"))
}

/// Drop target placing a dragged icon before `before` (or last) in `place`;
/// a thin line shows where it lands.
fn drop_slot(
    name: SharedString,
    place: BadgePlace,
    before: Option<&'static str>,
    cx: &App,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(name.clone()))
        .group(name.clone())
        .relative()
        .child(drag::insertion_mark::<DraggedBadge>(name, cx))
        .on_drop(move |dragged: &DraggedBadge, _, cx| {
            let known = extensions::badge_ids();
            prefs::update(cx, |preferences| {
                preferences
                    .extensions
                    .move_badge(&known, dragged.id, place, before)
            });
        })
}

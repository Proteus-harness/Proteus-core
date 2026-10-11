//! Rows of extension icons under the composer and in the window bars.

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::extensions::{self, DraggedBadge, Host};
use super::motion::Slide;
use super::prefs;
use super::workspace::Workspace;
use crate::preferences::BadgePlace;

mod arrange;

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

    /// Row of extension icons. While an icon is dragged the row parts
    /// where it would land, and a drop puts it there.
    pub(super) fn render_badge_strip(
        &self,
        place: BadgePlace,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let ids: Vec<&'static str> = extensions::visible_badges(place, cx)
            .into_iter()
            .map(|manifest| manifest.id)
            .filter(|id| self.badges.contains_key(id))
            .collect();
        let frame = arrange::frame(place, &ids, cx);
        let others: Vec<&'static str> = ids
            .iter()
            .copied()
            .filter(|id| Some(*id) != frame.dragging)
            .collect();
        let holds_source = others.len() != ids.len();
        let tag = slot_name(place);
        let generation = frame.generation;
        let gap = |index: usize| {
            Slide::new(
                SharedString::from(format!("badge-gap-{tag}-{index}-{generation}")),
                frame.open_gap == Some(index),
                div().w(frame.gap_width).h_7(),
            )
        };
        let mut row = Vec::new();
        let mut index = 0;
        for id in ids.iter().copied() {
            let source = Some(id) == frame.dragging;
            if !source && frame.dragging.is_some() {
                row.push(gap(index).into_any_element());
            }
            let view = self.badges[id].clone();
            row.push(
                Slide::new(
                    SharedString::from(format!("badge-item-{id}-{generation}")),
                    !source,
                    div()
                        .px(px(1.))
                        .on_prepaint(move |bounds, _, _| arrange::record(id, bounds))
                        .child(view),
                )
                .into_any_element(),
            );
            if !source {
                index += 1;
            }
        }
        if frame.dragging.is_some() {
            row.push(gap(index).into_any_element());
        }
        let hover_others = others.clone();
        h_flex()
            .id(SharedString::from(format!("badge-strip-{tag}")))
            .flex_none()
            .h_7()
            .items_center()
            .when(frame.dragging.is_some() && others.is_empty(), |strip| {
                strip.min_w(prefs::scaled(28.))
            })
            .children(row)
            .on_drag_move::<DraggedBadge>(move |event, window, _| {
                arrange::hover(
                    place,
                    &hover_others,
                    holds_source,
                    event.bounds,
                    event.event.position,
                    window,
                )
            })
            .on_drop(move |dragged: &DraggedBadge, _, cx| {
                let Some(index) = arrange::drop_index(place) else {
                    return;
                };
                let before = others
                    .iter()
                    .copied()
                    .filter(|id| *id != dragged.id)
                    .nth(index);
                let known = extensions::badge_ids();
                prefs::update(cx, |preferences| {
                    preferences
                        .extensions
                        .move_badge(&known, dragged.id, place, before)
                });
            })
    }
}

fn slot_name(place: BadgePlace) -> &'static str {
    match place {
        BadgePlace::Composer => "composer",
        BadgePlace::Header => "header",
        BadgePlace::Status => "status",
        BadgePlace::Hidden => "hidden",
    }
}

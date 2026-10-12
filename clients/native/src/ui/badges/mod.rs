//! Rows of extension icons at both ends of the composer row and the top
//! bar, and columns of them at both ends of the rail.

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::extensions::{self, DraggedBadge, Host};
use super::motion::{BASE, Slide, glide};
use super::prefs;
use super::workspace::Workspace;
use crate::preferences::BadgePlace;

mod arrange;

impl Workspace {
    /// Builds icons that became visible and drops the hidden ones, so an
    /// icon keeps its state while it moves between the strips.
    pub(super) fn sync_badges(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible: Vec<_> = BadgePlace::SHOWN
            .into_iter()
            .flat_map(|place| extensions::visible_badges(place, cx))
            .collect();
        let before = self.badges.len();
        self.badges
            .retain(|id, _| visible.iter().any(|manifest| manifest.id == *id));
        let mut changed = self.badges.len() != before;
        for manifest in visible {
            let Some(badge) = &manifest.badge else {
                continue;
            };
            if !self.badges.contains_key(manifest.id) {
                let view = (badge.build)(Host::new(cx.weak_entity(), manifest.id), window, cx);
                self.badges.insert(manifest.id, view);
                changed = true;
            }
        }
        // The composer strip is drawn inside the chat's cached view.
        if changed && let Some(chat) = self.active_chat_view() {
            chat.update(cx, |_, cx| cx.notify());
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
        let tag = slot_name(place);
        let generation = frame.generation;
        let vertical = place.is_rail();
        // While an icon is dragged every gap among the others is there; the
        // open one eases to the icon's length and holds the icon itself, so
        // the icons around it slide apart and its old place closes up.
        let dragged = frame.dragging.and_then(|id| self.badges.get(id).cloned());
        let gap = |index: usize| {
            let open = frame.open_gap == Some(index);
            Gap {
                id: SharedString::from(format!("badge-gap-{tag}-{index}-{generation}")).into(),
                open,
                length: frame.gap_length,
                vertical,
                icon: dragged
                    .clone()
                    .filter(|_| open)
                    .map(|view| item_box(vertical).child(view).into_any_element()),
            }
        };
        let mut row = Vec::new();
        let mut index = 0;
        for id in ids.iter().copied() {
            let source = Some(id) == frame.dragging;
            if !source && frame.dragging.is_some() {
                row.push(gap(index).into_any_element());
            }
            // The dragged icon is drawn in its gap; its own place is empty.
            let item = if source {
                div()
            } else {
                item_box(vertical)
                    .on_prepaint(move |bounds, _, _| arrange::record(id, bounds))
                    .child(self.badges[id].clone())
            };
            row.push(
                Slide::new(
                    SharedString::from(format!("badge-item-{id}-{generation}")),
                    !source,
                    item,
                )
                .vertical(vertical)
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
        let dragging = frame.dragging.is_some();
        let zone = dragging.then(|| drop_zone(place, frame.hovered, cx));
        let strip = if vertical {
            v_flex().min_w_7()
        } else {
            h_flex().h_7()
        };
        strip
            .id(SharedString::from(format!("badge-strip-{tag}")))
            .relative()
            .flex_none()
            .items_center()
            // An empty row offers room for exactly one icon.
            .when(dragging && others.is_empty(), |strip| {
                if vertical {
                    strip.min_h(frame.gap_length)
                } else {
                    strip.min_w(frame.gap_length)
                }
            })
            .children(zone)
            .children(row)
            .on_drag_move::<DraggedBadge>(move |event, window, _| {
                arrange::hover(
                    place,
                    &hover_others,
                    event.bounds,
                    event.event.position,
                    window,
                )
            })
            .when(dragging, |strip| strip.child(drop_catcher(place, others)))
    }
}

/// Holder of one icon, with a hairline of room along the row.
fn item_box(vertical: bool) -> Div {
    let item = div().flex_none();
    if vertical {
        item.py(px(1.))
    } else {
        item.px(px(1.))
    }
}

/// Room among the icons where the dragged one would land. The icon keeps
/// its size, clipped around its middle, and fades in while the gap opens;
/// a gap that shuts is empty.
#[derive(IntoElement)]
struct Gap {
    id: ElementId,
    open: bool,
    length: Pixels,
    vertical: bool,
    icon: Option<AnyElement>,
}

impl RenderOnce for Gap {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let share = glide(self.id, self.open, BASE, window, cx);
        let gap = if self.vertical {
            v_flex().min_w_7().h(self.length * share)
        } else {
            h_flex().h_7().w(self.length * share)
        };
        gap.flex_none()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .opacity(share * share)
            .children(self.icon)
    }
}

/// Outline of a row that takes the dragged icon, drawn under the icons
/// while any icon is dragged; brighter on the row it would land in.
fn drop_zone(place: BadgePlace, hovered: bool, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (fill, border) = if hovered {
        (theme.drop_target.opacity(0.35), theme.drag_border)
    } else {
        (
            theme.drop_target.opacity(0.12),
            theme.drag_border.opacity(0.45),
        )
    };
    let (x, y) = arrange::zone(place);
    div()
        .absolute()
        .top(-y)
        .bottom(-y)
        .left(-x)
        .right(-x)
        .rounded(theme.radius)
        .border_1()
        .border_color(border)
        .bg(fill)
}

/// Drop target reaching a little past the row on every side, laid over it
/// while an icon is dragged.
fn drop_catcher(place: BadgePlace, others: Vec<&'static str>) -> impl IntoElement {
    let (x, y) = arrange::reach(place);
    div()
        .absolute()
        .top(-y)
        .bottom(-y)
        .left(-x)
        .right(-x)
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

fn slot_name(place: BadgePlace) -> &'static str {
    match place {
        BadgePlace::ComposerStart => "composer-start",
        BadgePlace::ComposerEnd => "composer-end",
        BadgePlace::HeaderStart => "header-start",
        BadgePlace::HeaderEnd => "header-end",
        BadgePlace::RailStart => "rail-start",
        BadgePlace::RailEnd => "rail-end",
        BadgePlace::Hidden => "hidden",
    }
}

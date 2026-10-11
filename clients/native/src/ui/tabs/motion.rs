//! Motion of the tab area: tabs that open grow in and closed ones shrink
//! away, panes glide to their new places when the tree changes.
//!
//! The layout itself changes at once; this module only remembers where
//! things were and for how long they may still be animated.

use std::collections::HashMap;
use std::time::Instant;

use crate::tabs::{PaneId, Rect, TabLayout};
use crate::ui::motion::{self, BASE, FAST, Tween};

/// A closed tab still drawn while it shrinks away.
pub(super) struct Leaving {
    pub pane: PaneId,
    pub index: usize,
    pub id: String,
    pub pinned: bool,
    tween: Tween,
}

#[derive(Default)]
pub(in crate::ui) struct TabMotion {
    entering: HashMap<String, Tween>,
    leaving: Vec<Leaving>,
    panes: PaneMotion,
}

impl TabMotion {
    /// Notes the tabs that `new` opened or closed compared with `old`.
    pub(super) fn record(&mut self, old: &TabLayout, new: &TabLayout) {
        if !motion::plays() {
            return;
        }
        for pane in new.panes() {
            for tab in &pane.tabs {
                if !old.contains(tab) {
                    self.leaving.retain(|leaving| leaving.id != *tab);
                    self.entering.insert(tab.clone(), Tween::start(FAST));
                }
            }
        }
        for pane in old.panes() {
            if new.pane(pane.id).is_none() {
                continue;
            }
            for (index, tab) in pane.tabs.iter().enumerate() {
                if !new.contains(tab) {
                    self.entering.remove(tab);
                    self.leaving.push(Leaving {
                        pane: pane.id,
                        index,
                        id: tab.clone(),
                        pinned: index < pane.pinned,
                        tween: Tween::start(FAST),
                    });
                }
            }
        }
    }

    /// How far tab `id` has grown in, when it opened a moment ago.
    pub(super) fn entering(&self, id: &str, now: Instant) -> Option<f32> {
        let tween = self.entering.get(id)?;
        tween
            .running(now)
            .then(|| motion::ease_enter(tween.linear(now)))
    }

    /// Closed tabs of `pane` still shrinking, with what is left of them.
    pub(super) fn leaving(&self, pane: PaneId, now: Instant) -> Vec<(&Leaving, f32)> {
        self.leaving
            .iter()
            .filter(|leaving| leaving.pane == pane && leaving.tween.running(now))
            .map(|leaving| (leaving, 1. - motion::ease_exit(leaving.tween.linear(now))))
            .collect()
    }

    /// Whether a document tab opened a moment ago, so its content rises in.
    pub(super) fn opened(&self, id: &str, now: Instant) -> bool {
        self.entering
            .get(id)
            .is_some_and(|tween| tween.age(now) < BASE)
    }

    /// Drops finished transitions; true while any still runs.
    pub(super) fn tick(&mut self, now: Instant) -> bool {
        self.entering.retain(|_, tween| tween.age(now) < BASE);
        self.leaving.retain(|leaving| leaving.tween.running(now));
        !self.entering.is_empty() || !self.leaving.is_empty() || self.panes.running(now)
    }

    pub(super) fn panes(&mut self) -> &mut PaneMotion {
        &mut self.panes
    }
}

/// Places of the panes on screen while the tree changes.
#[derive(Default)]
pub(super) struct PaneMotion {
    /// Size of the tab area the places were computed for.
    size: Option<(f32, f32)>,
    target: Vec<(PaneId, Rect)>,
    from: Vec<(PaneId, Start)>,
    tween: Option<Tween>,
}

/// Where a pane starts gliding from.
#[derive(Clone, Copy)]
enum Start {
    /// It was on screen there.
    Moved(Rect),
    /// It is new and grows out of the pane it was split from.
    Born(Rect),
    /// It is new and has nowhere to grow from, so it only fades in.
    Fade,
}

/// A pane drawn during a transition.
pub(super) struct Frame {
    pub id: PaneId,
    pub rect: Rect,
    pub target: Rect,
    pub opacity: f32,
}

impl PaneMotion {
    fn running(&self, now: Instant) -> bool {
        self.tween.is_some_and(|tween| tween.running(now))
    }

    /// Takes the places the panes should end at. A different arrangement of
    /// the same area starts a glide from where the panes are now; a resized
    /// window or a dragged divider only moves them.
    pub(super) fn retarget(
        &mut self,
        size: (f32, f32),
        target: Vec<(PaneId, Rect)>,
        follow: bool,
        now: Instant,
    ) {
        let same_size = self.size == Some(size);
        let changed = !same_places(&self.target, &target);
        if same_size && !changed {
            return;
        }
        if follow || !same_size || self.target.is_empty() || !motion::moves() {
            self.tween = None;
        } else {
            let shown = self.frames(now);
            self.from = target
                .iter()
                .map(|(id, rect)| (*id, start(*id, *rect, &shown)))
                .collect();
            self.tween = Some(Tween::start(BASE));
        }
        self.size = Some(size);
        self.target = target;
    }

    /// The panes as drawn right now, or `None` when they are settled and
    /// lay out by themselves.
    pub(super) fn sample(&self, now: Instant) -> Option<Vec<Frame>> {
        self.running(now).then(|| self.frames(now))
    }

    fn frames(&self, now: Instant) -> Vec<Frame> {
        let progress = match self.tween {
            Some(tween) if tween.running(now) => motion::ease_enter(tween.linear(now)),
            _ => 1.,
        };
        self.target
            .iter()
            .map(|(id, target)| {
                let start = self
                    .from
                    .iter()
                    .find(|(from, _)| from == id)
                    .map_or(Start::Moved(*target), |(_, start)| *start);
                let (rect, opacity) = match start {
                    Start::Moved(from) => (lerp(from, *target, progress), 1.),
                    Start::Born(from) => (lerp(from, *target, progress), progress),
                    Start::Fade => (*target, progress),
                };
                Frame {
                    id: *id,
                    rect,
                    target: *target,
                    opacity,
                }
            })
            .collect()
    }
}

fn same_places(a: &[(PaneId, Rect)], b: &[(PaneId, Rect)]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|((a_id, a), (b_id, b))| {
            a_id == b_id
                && (a.x - b.x).abs() < 0.5
                && (a.y - b.y).abs() < 0.5
                && (a.w - b.w).abs() < 0.5
                && (a.h - b.h).abs() < 0.5
        })
}

/// Where pane `id`, ending at `target`, starts when `shown` is on screen.
fn start(id: PaneId, target: Rect, shown: &[Frame]) -> Start {
    if let Some(frame) = shown.iter().find(|frame| frame.id == id) {
        return Start::Moved(frame.rect);
    }
    let (cx, cy) = (target.x + target.w / 2., target.y + target.h / 2.);
    let Some(parent) = shown.iter().map(|frame| frame.rect).find(|rect| {
        cx >= rect.x && cx <= rect.x + rect.w && cy >= rect.y && cy <= rect.y + rect.h
    }) else {
        return Start::Fade;
    };
    let near = |a: f32, b: f32| (a - b).abs() < 2.;
    let right = target.x + target.w;
    let bottom = target.y + target.h;
    if near(right, parent.x + parent.w) && target.x > parent.x + 1. {
        Start::Born(Rect {
            x: right,
            w: 0.,
            ..target
        })
    } else if near(target.x, parent.x) && right < parent.x + parent.w - 1. {
        Start::Born(Rect { w: 0., ..target })
    } else if near(bottom, parent.y + parent.h) && target.y > parent.y + 1. {
        Start::Born(Rect {
            y: bottom,
            h: 0.,
            ..target
        })
    } else if near(target.y, parent.y) && bottom < parent.y + parent.h - 1. {
        Start::Born(Rect { h: 0., ..target })
    } else {
        Start::Fade
    }
}

fn lerp(from: Rect, to: Rect, t: f32) -> Rect {
    let mix = |a: f32, b: f32| a + (b - a) * t;
    Rect {
        x: mix(from.x, to.x),
        y: mix(from.y, to.y),
        w: mix(from.w, to.w),
        h: mix(from.h, to.h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    fn frame(id: u32, rect: Rect) -> Frame {
        Frame {
            id: PaneId(id),
            rect,
            target: rect,
            opacity: 1.,
        }
    }

    #[test]
    fn split_off_pane_grows_out_of_its_parent_edge() {
        let shown = [frame(1, rect(0., 0., 1000., 600.))];
        let right = start(PaneId(2), rect(500., 0., 500., 600.), &shown);
        assert!(matches!(right, Start::Born(r) if r == rect(1000., 0., 0., 600.)));
        let below = start(PaneId(2), rect(0., 300., 1000., 300.), &shown);
        assert!(matches!(below, Start::Born(r) if r == rect(0., 600., 1000., 0.)));
        let left = start(PaneId(2), rect(0., 0., 400., 600.), &shown);
        assert!(matches!(left, Start::Born(r) if r == rect(0., 0., 0., 600.)));
        let moved = start(PaneId(1), rect(0., 0., 500., 600.), &shown);
        assert!(matches!(moved, Start::Moved(r) if r == rect(0., 0., 1000., 600.)));
    }

    #[test]
    fn rearranging_glides_but_resizing_the_area_does_not() {
        let now = Instant::now();
        let mut panes = PaneMotion::default();
        let one = vec![(PaneId(1), rect(0., 0., 1000., 600.))];
        panes.retarget((1000., 600.), one.clone(), false, now);
        assert!(panes.sample(now).is_none());
        let two = vec![
            (PaneId(1), rect(0., 0., 497.5, 600.)),
            (PaneId(2), rect(502.5, 0., 497.5, 600.)),
        ];
        panes.retarget((1000., 600.), two.clone(), false, now);
        let frames = panes.sample(now).expect("a split glides");
        assert_eq!(frames[0].rect, rect(0., 0., 1000., 600.));
        assert_eq!(frames[1].opacity, 0.);
        panes.retarget((1200., 600.), two, false, now);
        assert!(panes.sample(now).is_none());
        panes.retarget((1200., 600.), one, true, now);
        assert!(panes.sample(now).is_none());
    }
}

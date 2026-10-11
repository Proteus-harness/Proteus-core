//! A horizontal reveal that eases itself open or shut, so a row can part
//! smoothly where something is about to land.

use std::time::Instant;

use gpui_kit::*;

use super::{Collapse, FAST};

/// Shows its child at full width when `open`, gliding there from where the
/// previous frames left it. The first frame shows the requested state at once.
#[derive(IntoElement)]
pub(in crate::ui) struct Slide {
    id: ElementId,
    open: bool,
    child: AnyElement,
}

struct SlideState {
    progress: f32,
    at: Option<Instant>,
}

impl Slide {
    pub(in crate::ui) fn new(
        id: impl Into<ElementId>,
        open: bool,
        child: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            open,
            child: child.into_any_element(),
        }
    }
}

impl RenderOnce for Slide {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let target = if self.open { 1. } else { 0. };
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| SlideState {
            progress: target,
            at: None,
        });
        let now = Instant::now();
        let progress = state.update(cx, |state, _| {
            if !super::moves() {
                state.progress = target;
            }
            if state.progress == target {
                state.at = None;
                return target;
            }
            let elapsed = state
                .at
                .map_or(0., |at| now.duration_since(at).as_secs_f32().min(0.05));
            state.at = Some(now);
            let step = elapsed / FAST.as_secs_f32();
            state.progress = if target > state.progress {
                (state.progress + step).min(target)
            } else {
                (state.progress - step).max(target)
            };
            state.progress
        });
        if progress != target {
            window.request_animation_frame();
        }
        let eased = progress * progress * (3. - 2. * progress);
        Collapse::new(self.id, eased, self.child)
    }
}

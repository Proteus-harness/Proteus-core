//! Numbers that glide to a new value instead of jumping.

use std::time::Duration;

use gpui_kit::*;

use super::moves;

const GLIDE: Duration = Duration::from_millis(450);

/// A formatted number; the first render shows the value as is.
#[derive(IntoElement)]
pub(in crate::ui) struct Number {
    id: ElementId,
    value: f32,
    format: fn(f32) -> String,
}

impl Number {
    pub(in crate::ui) fn new(
        id: impl Into<ElementId>,
        value: f32,
        format: fn(f32) -> String,
    ) -> Self {
        Self {
            id: id.into(),
            value,
            format,
        }
    }
}

impl RenderOnce for Number {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let shown = if moves() {
            gpui_kit::base::transition(
                self.id,
                self.value,
                gpui_kit::base::Transition::new(GLIDE),
                window,
                cx,
            )
        } else {
            self.value
        };
        (self.format)(shown)
    }
}

//! A loading spinner turning on the shared tick: every frame of a running
//! animation redraws the window, and a spinner reads as turning long before
//! the display's rate.

use std::time::Duration;

use gpui_kit::component::{Icon, IconName, Sizable, Size};
use gpui_kit::*;

use super::ticker;

const TURN: Duration = Duration::from_millis(800);

#[derive(IntoElement)]
pub(in crate::ui) struct Spinner {
    size: Size,
}

impl Spinner {
    pub(in crate::ui) fn new() -> Self {
        Self { size: Size::Medium }
    }
}

impl Sizable for Spinner {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl RenderOnce for Spinner {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let turn = if super::plays() {
            ticker::next_tick(window, cx);
            ease_in_out(ticker::phase(TURN))
        } else {
            0.
        };
        div().child(
            Icon::new(IconName::Loader)
                .with_size(self.size)
                .transform(Transformation::rotate(percentage(turn))),
        )
    }
}

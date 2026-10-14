//! The element under every disclosure. While the disclosure rests open the
//! child takes part in the surrounding layout and its height is remembered,
//! so a close starts from that height instead of collapsing to nothing for a
//! frame. While it moves, the child is laid out on its own and clipped to a
//! share of its height.

use gpui_kit::*;

pub(super) struct Reveal {
    id: ElementId,
    /// `None` while the disclosure rests open.
    progress: Option<f32>,
    child: AnyElement,
}

#[derive(Clone, Copy, Default)]
struct RevealState {
    height: Pixels,
}

impl Reveal {
    pub(super) fn new(id: impl Into<ElementId>, progress: Option<f32>, child: AnyElement) -> Self {
        Self {
            id: id.into(),
            progress: progress.map(|progress| progress.clamp(0., 1.)),
            child,
        }
    }
}

impl IntoElement for Reveal {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Reveal {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let Some(progress) = self.progress else {
            return (self.child.request_layout(window, cx), ());
        };
        let height = window.with_element_state(
            global_id.expect("Reveal has an id"),
            |state: Option<RevealState>, _| {
                let state = state.unwrap_or_default();
                (state.height, state)
            },
        );
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (height * progress).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let id = global_id.expect("Reveal has an id");
        if self.progress.is_none() {
            remember(id, bounds.size.height, window);
            self.child.prepaint(window, cx);
            return;
        }
        let measured = self.child.layout_as_root(
            size(
                AvailableSpace::Definite(bounds.size.width),
                AvailableSpace::MinContent,
            ),
            window,
            cx,
        );
        if remember(id, measured.height, window) {
            window.request_animation_frame();
        }
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            self.child.prepaint_at(bounds.origin, window, cx);
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.progress.is_none() {
            self.child.paint(window, cx);
            return;
        }
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            self.child.paint(window, cx);
        });
    }
}

/// Stores the child's height; true when it differs from the stored one.
fn remember(id: &GlobalElementId, height: Pixels, window: &mut Window) -> bool {
    window.with_element_state(id, |state: Option<RevealState>, _| {
        let changed = state.is_none_or(|state| state.height != height);
        (changed, RevealState { height })
    })
}

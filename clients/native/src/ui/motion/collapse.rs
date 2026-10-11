//! A horizontal reveal: the child keeps its natural width and is clipped to
//! a share of it, so strips (tabs) open and close smoothly.

use gpui_kit::*;

/// Shows `progress` (0..=1) of the child's natural width.
pub(in crate::ui) struct Collapse {
    id: ElementId,
    progress: f32,
    child: AnyElement,
}

#[derive(Clone, Copy, Default)]
struct CollapseState {
    width: Option<Pixels>,
}

impl Collapse {
    pub(in crate::ui) fn new(id: impl Into<ElementId>, progress: f32, child: AnyElement) -> Self {
        Self {
            id: id.into(),
            progress: progress.clamp(0., 1.),
            child,
        }
    }
}

impl IntoElement for Collapse {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Collapse {
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
        let width = window.with_element_state(
            global_id.expect("Collapse has an id"),
            |state: Option<CollapseState>, _| {
                let state = state.unwrap_or_default();
                (state.width, state)
            },
        );
        let mut style = Style::default();
        style.size.height = relative(1.).into();
        style.flex_shrink = 0.;
        style.size.width = match width {
            Some(width) => (width * self.progress).into(),
            None => px(0.).into(),
        };
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
        let measured = self.child.layout_as_root(
            size(
                AvailableSpace::MaxContent,
                AvailableSpace::Definite(bounds.size.height),
            ),
            window,
            cx,
        );
        let changed = window.with_element_state(
            global_id.expect("Collapse has an id"),
            |state: Option<CollapseState>, _| {
                let mut state = state.unwrap_or_default();
                let changed = state.width != Some(measured.width);
                state.width = Some(measured.width);
                (changed, state)
            },
        );
        if changed {
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
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            self.child.paint(window, cx);
        });
    }
}

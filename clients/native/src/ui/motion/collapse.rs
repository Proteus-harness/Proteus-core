//! A horizontal reveal: the child keeps its natural width and is clipped to
//! a share of it, so strips (tabs) open and close smoothly. A squeezable
//! reveal also gives way when its row runs out of room.

use gpui_kit::*;

/// Shows `progress` (0..=1) of the child's natural width.
pub(in crate::ui) struct Collapse {
    id: ElementId,
    progress: f32,
    child: AnyElement,
    squeeze: Option<Squeeze>,
}

/// How far a crowded row may narrow the child.
#[derive(Clone, Copy)]
struct Squeeze {
    min: AbsoluteLength,
    basis: AbsoluteLength,
}

#[derive(Clone, Copy)]
struct CollapseState {
    width: Pixels,
}

impl Collapse {
    pub(in crate::ui) fn new(id: impl Into<ElementId>, progress: f32, child: AnyElement) -> Self {
        Self {
            id: id.into(),
            progress: progress.clamp(0., 1.),
            child,
            squeeze: None,
        }
    }

    /// Lets a crowded row squeeze the child down to `min`; the child is then
    /// laid out at the narrower width, so it should fill the width it gets.
    /// Siblings with the same `basis`, at least their natural width, give up
    /// room evenly from the widest down, so short ones stay whole longest.
    pub(in crate::ui) fn squeeze(
        mut self,
        min: impl Into<AbsoluteLength>,
        basis: impl Into<AbsoluteLength>,
    ) -> Self {
        self.squeeze = Some(Squeeze {
            min: min.into(),
            basis: basis.into(),
        });
        self
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
        let child = &mut self.child;
        let width = window.with_element_state(
            global_id.expect("Collapse has an id"),
            |state: Option<CollapseState>, window| {
                // A child new under this id, such as a tab dropped into a
                // strip or a row drawn afresh after a drag, is measured right
                // away instead of taking no room for a frame.
                let width = state.map_or_else(
                    || {
                        let space = size(AvailableSpace::MaxContent, AvailableSpace::MaxContent);
                        child.layout_as_root(space, window, cx).width
                    },
                    |state| state.width,
                );
                (width, CollapseState { width })
            },
        );
        let mut style = Style::default();
        style.size.height = relative(1.).into();
        style.flex_shrink = 0.;
        style.size.width = (width * self.progress).into();
        if let Some(squeeze) = self.squeeze {
            // Equal bases shrink by equal amounts, and the natural width as
            // the maximum keeps a short child whole until the others reach it.
            let rem = window.rem_size();
            let basis = squeeze.basis.to_pixels(rem).max(width);
            let min = squeeze.min.to_pixels(rem).min(width);
            style.flex_shrink = 1.;
            style.flex_basis = (basis * self.progress).into();
            style.size.width = Length::Auto;
            style.max_size.width = (width * self.progress).into();
            style.min_size.width = (min * self.progress).into();
        }
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
                let changed = state.is_none_or(|state| state.width != measured.width);
                let width = measured.width;
                (changed, CollapseState { width })
            },
        );
        if changed {
            window.request_animation_frame();
        }
        if self.squeeze.is_some() && self.progress > 0. {
            let room = bounds.size.width / self.progress;
            if room < measured.width - px(0.5) {
                self.child.layout_as_root(
                    size(
                        AvailableSpace::Definite(room),
                        AvailableSpace::Definite(bounds.size.height),
                    ),
                    window,
                    cx,
                );
            }
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

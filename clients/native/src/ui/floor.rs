//! Scrolled content that keeps its height while a block in it closes under a
//! scrolled-down viewport. Without it the scroll position runs past the new
//! end, gets clamped and the whole page jumps; with it the freed room stays
//! empty below the content and goes away as the user scrolls back up.

use gpui_kit::*;

/// Content of a vertical scroll area, centred horizontally.
pub(in crate::ui) struct Floor {
    id: ElementId,
    child: AnyElement,
}

#[derive(Clone, Copy, Default)]
struct FloorState {
    floor: Pixels,
}

pub(in crate::ui) fn floor(id: impl Into<ElementId>, child: impl IntoElement) -> Floor {
    Floor {
        id: id.into(),
        child: child.into_any_element(),
    }
}

impl IntoElement for Floor {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Floor {
    type RequestLayoutState = LayoutId;
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
        let floor = window.with_element_state(
            global_id.expect("Floor has an id"),
            |state: Option<FloorState>, _| {
                let state = state.unwrap_or_default();
                (state.floor, state)
            },
        );
        let child = self.child.request_layout(window, cx);
        let mut style = Style::default();
        style.display = Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.align_items = Some(AlignItems::Center);
        style.size.width = relative(1.).into();
        style.min_size.height = floor.into();
        (window.request_layout(style, [child], cx), child)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let natural = window.layout_bounds(*child).size.height;
        // The part of the content above the viewport's bottom edge must stay,
        // or the scroll position would be clamped; the rest may go.
        let visible = (window.content_mask().bounds.bottom() - bounds.top()).max(px(0.));
        let floor = natural.max(visible.min(bounds.size.height));
        window.with_element_state(
            global_id.expect("Floor has an id"),
            |_: Option<FloorState>, _| ((), FloorState { floor }),
        );
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

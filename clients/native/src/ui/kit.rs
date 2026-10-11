//! gpui-kit controls as the client builds them. A press of any mouse button
//! focuses a control, and the kit then rings it as if it was reached with
//! Tab; the client's controls keep focus without the ring.

use gpui_kit::ElementId;
use gpui_kit::component::FocusableExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::switch::Switch;

pub(crate) fn button(id: impl Into<ElementId>) -> Button {
    Button::new(id).focus_ring(false)
}

pub(crate) fn switch(id: impl Into<ElementId>) -> Switch {
    Switch::new(id).focus_ring(false)
}

pub(crate) fn checkbox(id: impl Into<ElementId>) -> Checkbox {
    Checkbox::new(id).focus_ring(false)
}

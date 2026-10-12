//! What is being dragged right now, so its own place can fade while the
//! preview follows the cursor, and the look of that preview.

use gpui_kit::component::*;
use gpui_kit::*;

/// Key of the dragged element, e.g. `tab:chat` or `badge:plan`.
#[derive(Default)]
struct DragSource(Option<SharedString>);

impl Global for DragSource {}

/// Remembers the element a drag starts from; call in the drag constructor.
pub(super) fn start(key: impl Into<SharedString>, cx: &mut App) {
    cx.set_global(DragSource(Some(key.into())));
}

/// Whether the element with `key` is being dragged.
pub(super) fn is_source(key: &str, cx: &App) -> bool {
    current(cx).is_some_and(|source| source == key)
}

/// Key of what is being dragged, if anything.
pub(super) fn current(cx: &App) -> Option<&str> {
    if !cx.has_active_drag() {
        return None;
    }
    cx.try_global::<DragSource>()?.0.as_deref()
}

/// Opacity of the place an element was lifted from.
pub(super) const SOURCE_OPACITY: f32 = 0.35;

/// Lifted look of a dragged copy: raised above the content with a shadow.
pub(super) fn lifted(element: Div, cx: &App) -> Div {
    let theme = cx.theme();
    element
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .text_color(theme.popover_foreground)
        .shadow_lg()
        .opacity(0.95)
}

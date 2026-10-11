//! Interface scale picker in the status bar.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::*;

use crate::preferences::SCALES;
use crate::ui::prefs;

pub(super) fn zoom_button(cx: &App) -> impl IntoElement + use<> {
    let current = prefs::current(cx).view.scale;
    Button::new("status-zoom")
        .ghost()
        .small()
        .label(format!("{current}%"))
        .tooltip("Масштаб интерфейса · Ctrl + =, Ctrl + −, Ctrl + 0")
        .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
            SCALES.iter().fold(menu, |menu, &scale| {
                menu.item(
                    PopupMenuItem::new(format!("{scale}%"))
                        .checked(scale == current)
                        .on_click(move |_, _, cx| {
                            prefs::update(cx, |prefs| prefs.view.scale = scale)
                        }),
                )
            })
        })
}

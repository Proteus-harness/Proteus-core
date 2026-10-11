//! Window chrome around the tabs: the top bar with the logo menu, the open
//! folder and extension icons, and the status bar at the bottom.

mod logo;
mod projects;
mod status;
mod top;
mod zoom;

use gpui_kit::component::Size;
use gpui_kit::{App, px};

/// Height of the top and status bars before zoom.
const BAR_HEIGHT: f32 = 34.;

/// Icon buttons of both bars: 24 px with an 18 px icon at 100%.
fn icon_button_size(cx: &App) -> Size {
    Size::Size(px(24. * crate::ui::prefs::scale(cx)))
}

pub(super) use projects::{Projects, WorkspaceRequest};
pub(super) use top::picker_top_bar;

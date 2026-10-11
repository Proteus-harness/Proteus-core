//! Window chrome around the tabs: the top bar with the logo menu, the open
//! folder and extension icons, and the status bar at the bottom.

mod logo;
mod projects;
mod status;
mod title;
mod top;

/// Height of the top and status bars before zoom.
const BAR_HEIGHT: f32 = 34.;

pub(super) use projects::{Projects, WorkspaceRequest};
pub(super) use top::picker_top_bar;

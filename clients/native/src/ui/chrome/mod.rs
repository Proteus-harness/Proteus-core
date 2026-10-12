//! Window chrome around the tabs: the top bar with the logo menu, the open
//! folder and extension icons, and the rail along the left edge.

mod logo;
mod projects;
mod rail;
mod title;
mod top;

/// Height of the top bar before zoom.
const BAR_HEIGHT: f32 = 34.;

pub(super) use projects::{Projects, WorkspaceRequest};
pub(super) use top::picker_top_bar;

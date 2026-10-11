//! Window chrome around the tabs: the top bar with the logo menu, the open
//! folder and extension icons, and the status bar at the bottom.

mod logo;
mod projects;
mod status;
mod top;

pub(super) use projects::{Projects, WorkspaceRequest};
pub(super) use top::picker_top_bar;

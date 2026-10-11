//! Headless analytics over the agent's public read API: request usage and
//! cost, the context window, turn analysis and the build topology.
//!
//! Only `proteus-contracts` DTOs and plain data cross this boundary; the
//! views in `ui::analytics` render what these modules compute.

pub mod analysis;
pub mod context;
pub mod format;
pub mod graph;
pub mod usage;

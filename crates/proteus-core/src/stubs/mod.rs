//! Host-owned structural absence and test implementations. Structural objects
//! are not registered as modules and have no module ids.

mod deny_all_policy;
mod empty_context;
mod no_workflow;

pub use deny_all_policy::*;
pub use empty_context::*;
pub use no_workflow::*;

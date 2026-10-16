//! Extensions installed without rebuilding the client.
//!
//! An extension is a folder with `extension.json` and any program speaking
//! [`protocol`] over stdio. The program gets the server origin and token and
//! talks to the agent itself; the client only renders the [`widget`] trees it
//! sends and forwards user actions, so the client stays independent of what
//! the extension does.

pub mod graph;
pub mod manifest;
pub mod package;
pub mod presentation;
pub mod process;
pub mod protocol;
pub mod session;
pub mod tree;
pub mod widget;

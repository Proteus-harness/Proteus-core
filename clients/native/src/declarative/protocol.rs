//! JSON lines between the client and an extension process.
//!
//! The client says hello once, then asks for views and forwards user actions
//! and host events. The process answers with whole view trees whenever it
//! wants: after a request, an action or on its own timer.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::widget::Node;

pub const PROTOCOL: u32 = 1;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outgoing {
    /// First message: how to reach the agent server on its own.
    Hello {
        protocol: u32,
        workspace: PathBuf,
        server: Server,
    },
    /// A surface was shown; the process should send its view.
    Render {
        view: String,
        surface: SurfaceKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        argument: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_dir: Option<PathBuf>,
    },
    /// The surface is gone; its view is no longer needed.
    Close { view: String },
    /// The user pressed a button or a list row.
    Action {
        view: String,
        action: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<Value>,
    },
    /// Something changed in the client, e.g. another chat became active.
    Event {
        event: HostEventKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_dir: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Server {
    pub origin: String,
    pub token: String,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    Panel,
    Document,
    Badge,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostEventKind {
    SessionOpened,
    RunSettled,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Incoming {
    /// The whole tree of one view.
    View { view: String, root: Node },
    /// Opens a document of this extension as a tab.
    OpenDocument { argument: String },
}

/// Parses one line from the process.
pub fn parse(line: &str) -> anyhow::Result<Incoming> {
    let message: Incoming = serde_json::from_str(line)?;
    if let Incoming::View { root, .. } = &message {
        root.validate()?;
    }
    Ok(message)
}

/// View id of a surface; documents are told apart by their argument.
pub fn view_id(surface: SurfaceKind, argument: Option<&str>) -> String {
    match (surface, argument) {
        (SurfaceKind::Panel, _) => "panel".into(),
        (SurfaceKind::Badge, _) => "badge".into(),
        (SurfaceKind::Document, argument) => format!("document:{}", argument.unwrap_or("")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_messages_as_tagged_json() {
        let render = Outgoing::Render {
            view: view_id(SurfaceKind::Document, Some("a")),
            surface: SurfaceKind::Document,
            argument: Some("a".into()),
            session_dir: None,
        };
        assert_eq!(
            serde_json::to_value(render).unwrap(),
            json!({"type": "render", "view": "document:a", "surface": "document", "argument": "a"})
        );
    }

    #[test]
    fn reads_views_and_rejects_invalid_trees() {
        let view = parse(r#"{"type": "view", "view": "panel", "root": {"type": "divider"}}"#);
        assert!(matches!(view, Ok(Incoming::View { .. })));
        assert!(parse(r#"{"type": "view", "view": "panel", "root": {"type": "nope"}}"#).is_err());
        assert!(parse(r#"{"type": "log", "text": "x"}"#).is_err());
    }
}

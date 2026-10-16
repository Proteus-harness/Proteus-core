//! JSON lines between the client and an extension process.
//!
//! The client says hello once, then asks for views and forwards user actions
//! and host events. The process answers with whole view trees whenever it
//! wants: after a request, an action or on its own timer.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::presentation::Face;
use super::widget::Node;

pub const PROTOCOL: u32 = 2;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outgoing {
    CommandResult {
        id: String,
        session_dir: PathBuf,
        config: Option<Box<proteus_contracts::app_protocol::config::ConfigSummary>>,
        error: Option<String>,
    },
    SessionSnapshot {
        session_dir: Option<PathBuf>,
        state: Option<Box<super::session::State>>,
        entries: Vec<super::session::Entry>,
    },
    SessionUpdate {
        session_dir: PathBuf,
        update: super::session::Update,
    },
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
        sequence: u64,
        view: String,
        action: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<Value>,
    },
    /// Something changed in the client, e.g. another chat became active.
    Event {
        event: HostEventKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        document: Option<String>,
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
    Page,
    Panel,
    Document,
    Badge,
    Popup,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostEventKind {
    SessionOpened,
    RunSettled,
    NavigationChanged,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Incoming {
    Focus {
        view: String,
        control: Option<String>,
    },
    ClosePopup,
    Clipboard {
        text: String,
    },
    SetSession {
        id: String,
        session_dir: PathBuf,
        setting: SessionSetting,
    },
    /// Icon/ring/text and full hover details, bound to the current session.
    Badge {
        #[serde(default)]
        hidden: bool,
        view: String,
        session_dir: Option<PathBuf>,
        title: String,
        lines: Vec<String>,
        face: Face,
    },
    /// The whole tree of one view.
    View {
        #[serde(default)]
        keys: std::collections::HashMap<String, String>,
        session_dir: Option<PathBuf>,
        view: String,
        root: Node,
        #[serde(default)]
        sequence: u64,
    },
    /// Opens a document of this extension as a tab.
    OpenDocument {
        argument: String,
        #[serde(default)]
        preview: bool,
        #[serde(default = "yes")]
        focus: bool,
    },
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionSetting {
    Mode {
        value: proteus_contracts::domain::PermissionMode,
    },
    Model {
        value: String,
    },
    Effort {
        value: Option<String>,
    },
}

/// Parses one line from the process.
pub fn parse(line: &str) -> anyhow::Result<Incoming> {
    let message: Incoming = serde_json::from_str(line)?;
    if let Incoming::View { root, .. } = &message {
        root.validate()?;
    }
    if let Incoming::Badge { face, .. } = &message {
        face.validate()?;
    }
    Ok(message)
}

/// View id of a surface; documents are told apart by their argument.
pub fn view_id(surface: SurfaceKind, argument: Option<&str>) -> String {
    match (surface, argument) {
        (SurfaceKind::Panel, _) => "panel".into(),
        (SurfaceKind::Page, _) => "page".into(),
        (SurfaceKind::Badge, _) => "badge".into(),
        (SurfaceKind::Popup, _) => "popup".into(),
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

    #[test]
    fn validates_independent_badge_presentations() {
        let badge = r#"{"type":"badge","view":"badge","session_dir":"/a","title":"Left 8%","lines":["Weekly"],"face":{"type":"ring","percent":92,"tone":"danger"}}"#;
        assert!(matches!(parse(badge).unwrap(), Incoming::Badge { .. }));
        assert!(parse(&badge.replace("\"percent\":92", "\"percent\":101")).is_err());
        assert!(parse(&badge.replace("\"type\":\"ring\"", "\"type\":\"unknown\"")).is_err());
    }

    #[test]
    fn shipped_processes_conform_to_the_public_ui_protocol() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../extensions");
        let (packages, errors) = crate::declarative::manifest::discover(&root.join("packages"));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(packages.len(), 12);
        let output = std::process::Command::new("python3")
            .arg(root.join("tests/wire.py"))
            .arg("--wire")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let wire = String::from_utf8(output.stdout).unwrap();
        let mut count = 0;
        for line in wire.lines() {
            parse(line).unwrap_or_else(|error| panic!("invalid extension output: {error}: {line}"));
            count += 1;
        }
        assert!(count >= 30, "not all declared surfaces were exercised");
    }
}

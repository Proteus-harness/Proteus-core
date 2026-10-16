//! Adapter from the current client model to the public extension projection.

use crate::declarative::session::{self, Update};
use crate::transcript::{Change, Role};
use crate::ui::{chat::Session, workspace::Workspace};
use gpui_kit::*;
use proteus_contracts::app_protocol::AppSessionActivity;
use std::path::PathBuf;

#[derive(Clone)]
pub(in crate::ui) struct SessionChanged {
    pub session_dir: PathBuf,
    pub update: Update,
}

impl EventEmitter<SessionChanged> for Workspace {}

pub(in crate::ui) fn state(
    session: &Session,
    activity: Option<AppSessionActivity>,
) -> session::State {
    session::State {
        connected: session.connected,
        config: session.config.clone(),
        context: session.context.map(|context| session::Context {
            used: context.used,
            max: context.max,
            trigger: context.trigger,
        }),
        execution: session.execution.clone(),
        pending: session.pending.clone(),
        activity,
    }
}

pub(in crate::ui) fn entries(
    session: &Session,
    range: std::ops::Range<usize>,
) -> Vec<session::Entry> {
    session.transcript.entries()[range]
        .iter()
        .map(|entry| session::Entry {
            role: match entry.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
            },
            tool: entry.tool.clone(),
            subagent: entry.subagent.clone(),
        })
        .collect()
}

pub(in crate::ui) fn changed(session: &Session, change: Change) -> Option<Update> {
    let length = session.transcript.entries().len();
    let (start, removed, inserted) = match change {
        Change::None => return None,
        Change::Pushed => (length.checked_sub(1)?, 0, 1),
        Change::Spliced {
            start,
            removed,
            inserted,
        } => (start, removed, inserted),
        Change::Updated(row) => {
            let entry = session.transcript.entries().get(row)?;
            // Text deltas leave this projection unchanged.
            if entry.tool.is_none() && entry.subagent.is_none() {
                return None;
            }
            (row, 1, 1)
        }
    };
    Some(Update::Entries {
        start,
        removed,
        entries: entries(session, start..start + inserted),
    })
}

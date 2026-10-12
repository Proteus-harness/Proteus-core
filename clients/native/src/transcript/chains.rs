//! Runs of consecutive tool calls that the chat may fold into one row.

use std::ops::Range;

use super::{Entry, Role};

/// Shorter runs stay expanded: folding two calls saves nothing.
pub const MIN_CHAIN: usize = 3;

fn is_tool(entry: &Entry) -> bool {
    entry.role == Role::System && entry.tool.is_some()
}

/// The run of tool rows containing `ix`, when it is long enough to fold.
pub fn tool_chain(entries: &[Entry], ix: usize) -> Option<Range<usize>> {
    tool_run(entries, ix).filter(|run| run.len() >= MIN_CHAIN)
}

/// Rows whose look may change when row `ix` changes: the whole run of tool
/// rows around it, since a growing run can start folding.
pub fn affected_rows(entries: &[Entry], ix: usize) -> Range<usize> {
    tool_run(entries, ix).unwrap_or(ix..(ix + 1).min(entries.len()))
}

fn tool_run(entries: &[Entry], ix: usize) -> Option<Range<usize>> {
    if !entries.get(ix).is_some_and(is_tool) {
        return None;
    }
    let start = entries[..ix]
        .iter()
        .rposition(|entry| !is_tool(entry))
        .map_or(0, |before| before + 1);
    let end = entries[ix..]
        .iter()
        .position(|entry| !is_tool(entry))
        .map_or(entries.len(), |after| ix + after);
    Some(start..end)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use proteus_contracts::app_protocol::AppTranscriptTool;

    use super::*;

    fn entry(key: u64, tool: bool) -> Entry {
        let tool = tool.then(|| AppTranscriptTool {
            call_id: format!("c{key}"),
            name: "read_file".into(),
            args: serde_json::Value::Null,
            effective_args: None,
            status: "done".into(),
            result: None,
            metadata: serde_json::Value::Null,
        });
        Entry {
            key,
            message_id: None,
            role: if tool.is_some() {
                Role::System
            } else {
                Role::Assistant
            },
            text: Arc::from(""),
            images: Vec::new(),
            tool,
            subagent: None,
            streaming: false,
        }
    }

    #[test]
    fn folds_only_long_runs_of_tools() {
        let rows = [
            entry(0, false),
            entry(1, true),
            entry(2, true),
            entry(3, false),
            entry(4, true),
            entry(5, true),
            entry(6, true),
        ];
        assert_eq!(tool_chain(&rows, 0), None);
        assert_eq!(tool_chain(&rows, 1), None);
        assert_eq!(tool_chain(&rows, 5), Some(4..7));
        assert_eq!(affected_rows(&rows, 6), 4..7);
        assert_eq!(affected_rows(&rows, 3), 3..4);
    }
}

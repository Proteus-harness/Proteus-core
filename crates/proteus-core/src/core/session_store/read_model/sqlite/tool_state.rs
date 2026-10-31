//! Compact index recovery reads only a source-valid display, one cached row at a time.
use super::*;
use crate::core::session_store::read_cache::Updater;
use proteus_contracts::app_protocol::{AppLatestToolState, AppToolState};

pub(super) const TOOL_STATE: &str = "tool-state";
const INDEX: &str = "tool-state-index";

pub(super) fn accepted(status: &str) -> bool {
    !matches!(status, "failed" | "denied")
}

pub(in crate::core::session_store::read_model) fn tool_state(
    store: &SessionStore,
    name: &str,
) -> Option<AppLatestToolState> {
    load(store, name).or_else(|| rebuild(store, name))
}

fn load(store: &SessionStore, name: &str) -> Option<AppLatestToolState> {
    let reader = Reader::open(store.session_dir(), VIEW_CACHE)?;
    let header: Header = reader.one("header")?;
    let fingerprint: (usize, [u8; 32]) = reader.one(INDEX)?;
    if !matches(store, &header) || reader.keyed_fingerprint(TOOL_STATE)? != fingerprint {
        return None;
    }
    let tool: Option<AppToolState> = reader.keyed(TOOL_STATE, name)?;
    if tool
        .as_ref()
        .is_some_and(|tool| tool.name != name || !accepted(&tool.status))
    {
        return None;
    }
    let (source, fresh) = reader.finish()?;
    fresh.then(|| {
        Some(AppLatestToolState {
            revision: source.revision()?,
            tool,
        })
    })?
}

// Ignore full results/text/images/metadata during decoding. The reader verifies
// each complete row checksum before this compact projection is deserialized.
#[derive(Deserialize)]
struct CompactRow {
    tool: Option<CompactTool>,
}

#[derive(Deserialize)]
struct CompactTool {
    call_id: String,
    name: String,
    status: String,
    args: serde_json::Value,
    effective_args: Option<serde_json::Value>,
}

impl CompactTool {
    fn into_state(self) -> AppToolState {
        AppToolState {
            call_id: self.call_id,
            name: self.name,
            status: self.status,
            args: self.args,
            effective_args: self.effective_args,
        }
    }
}

/// An already certified append may upgrade its exact prior cached prefix.
/// Missing auxiliary metadata must not force a full canonical projection.
pub(super) fn ensure_index(writer: &Updater, transcript_len: usize) -> Option<()> {
    if let Some(fingerprint) = writer.previous::<(usize, [u8; 32])>(INDEX)
        && writer.keyed_fingerprint(TOOL_STATE)? == fingerprint
    {
        return Some(());
    }
    writer.clear_keyed(TOOL_STATE)?;
    for position in 0..transcript_len {
        let row: CompactRow = writer
            .page("transcript", position, position.checked_add(1)?)?
            .pop()?;
        if let Some(row) = row.tool
            && accepted(&row.status)
        {
            let state = row.into_state();
            writer.keyed(TOOL_STATE, &state.name, &state)?;
        }
    }
    writer.one(INDEX, &writer.keyed_fingerprint(TOOL_STATE)?)
}

fn rebuild(store: &SessionStore, name: &str) -> Option<AppLatestToolState> {
    let source = SourceFingerprint::read(store.session_dir(), 8192)?;
    let reader = Reader::open(store.session_dir(), VIEW_CACHE)?;
    let header: Header = reader.one("header")?;
    if !matches(store, &header) || reader.revision()? != source.revision()? {
        return None;
    }
    // Source equality, row checksums and the final fingerprint authorize only
    // a disposable index rebuild; they do not authorize canonical execution.
    let writer = Updater::new(store.session_dir(), VIEW_CACHE, &source)?;
    if !writer.source_matches(&source) {
        return None;
    }
    writer.clear_keyed(TOOL_STATE)?;
    let mut tool = None;
    for position in 0..header.transcript_len {
        let row: CompactRow = reader
            .page("transcript", position, position.checked_add(1)?)?
            .pop()?;
        if let Some(row) = row.tool
            && accepted(&row.status)
        {
            let state = row.into_state();
            writer.keyed(TOOL_STATE, &state.name, &state)?;
            if state.name == name {
                tool = Some(state);
            }
        }
    }
    let (cached_source, fresh) = reader.finish()?;
    if !fresh || cached_source != source {
        return None;
    }
    let fingerprint = writer.keyed_fingerprint(TOOL_STATE)?;
    writer.one(INDEX, &fingerprint)?;
    writer.finish()?;
    Some(AppLatestToolState {
        revision: source.revision()?,
        tool,
    })
}

//! Private, strictly narrower wire reader for one staged text Append.
//! This is not a storage or journal format; rejection always retries the
//! original JournalRecord decoder on exactly the same complete staged line.
use super::super::super::plain_history::{OnlyNull, PlainHistoryMessage};
use crate::{
    domain::{ExecutionId, MessageId, PartId, RecordId, SessionId, ThreadId, TurnId},
    model_standard::{MessagePhase, MessageRole, PartScope},
};
use serde::{
    Deserialize, Deserializer,
    de::{Error, MapAccess, SeqAccess, Visitor},
};
use std::{fmt, num::NonZeroU64};

pub(super) const MIN_ROWS: usize = 4_096;
pub(super) const MAX_ROWS: usize = 65_536;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    #[serde(rename = "schema_version")]
    _schema_version: u32,
    #[serde(rename = "record_id")]
    _record_id: RecordId,
    #[serde(rename = "session_seq")]
    _session_seq: NonZeroU64,
    timestamp_ms: i64,
    #[serde(rename = "session_id")]
    _session_id: SessionId,
    #[serde(rename = "execution_id")]
    _execution_id: Option<ExecutionId>,
    #[serde(rename = "thread_id")]
    _thread_id: Option<ThreadId>,
    turn_id: Option<TurnId>,
    entry: Entry,
}

// Require the local serializer's tag-first object order, so Serde never
// buffers an arbitrary payload as generic Content for this fast path. Other
// equivalent JSON spellings still take the full authoritative decoder.
struct Entry(Append);

impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TagFirst;
        impl<'de> Visitor<'de> for TagFirst {
            type Value = Entry;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a tag-first staged HistoryMutated entry")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Entry, A::Error> {
                if map.next_key::<&str>()? != Some("kind")
                    || map.next_value::<&str>()? != "history_mutated"
                    || map.next_key::<&str>()? != Some("payload")
                {
                    return Err(A::Error::custom("not a tag-first staged Append"));
                }
                let payload = map.next_value()?;
                if map.next_key::<&str>()?.is_some() {
                    return Err(A::Error::custom("extra staged history entry fields"));
                }
                Ok(Entry(payload))
            }
        }
        deserializer.deserialize_map(TagFirst)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Append {
    #[serde(rename = "previous_revision")]
    _previous_revision: u64,
    #[serde(rename = "new_revision")]
    _new_revision: u64,
    #[serde(rename = "mutation")]
    _mutation: Mutation,
    messages: Rows,
    #[serde(rename = "compaction")]
    _compaction: Option<OnlyNull>,
    #[serde(rename = "tool_results")]
    _tool_results: [(); 0],
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mutation {
    Append,
}

struct Rows(Vec<PlainHistoryMessage>);

impl<'de> Deserialize<'de> for Rows {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BoundedRows;
        impl<'de> Visitor<'de> for BoundedRows {
            type Value = Rows;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "at most {MAX_ROWS} staged text rows")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Rows, A::Error> {
                let mut rows = Vec::new();
                while rows.len() < MAX_ROWS {
                    let Some(row) = seq.next_element()? else {
                        return Ok(Rows(rows));
                    };
                    rows.push(row);
                }
                if seq.next_element::<PlainHistoryMessage>()?.is_some() {
                    return Err(A::Error::custom("large staged text row list"));
                }
                Ok(Rows(rows))
            }
        }
        deserializer.deserialize_seq(BoundedRows)
    }
}

pub(crate) struct StagedTextAppend {
    pub(crate) timestamp_ms: i64,
    pub(super) turn_id: Option<TurnId>,
    messages: Vec<PlainHistoryMessage>,
}

pub(crate) struct StagedTextRow {
    pub(crate) id: MessageId,
    pub(crate) role: MessageRole,
    pub(crate) phase: Option<MessagePhase>,
    pub(crate) part_id: PartId,
    pub(crate) scope: PartScope,
    pub(crate) text: String,
}

impl StagedTextAppend {
    pub(super) fn decode(line: &[u8]) -> Option<Self> {
        let record = serde_json::from_slice::<Record>(line).ok()?;
        let Entry(append) = record.entry;
        let messages = append.messages.0;
        if messages.len() < MIN_ROWS {
            return None;
        }
        Some(Self {
            timestamp_ms: record.timestamp_ms,
            turn_id: record.turn_id,
            messages,
        })
    }

    pub(super) fn identities(&self) -> impl Iterator<Item = (MessageId, PartId)> + '_ {
        self.messages.iter().map(PlainHistoryMessage::identities)
    }

    pub(crate) fn into_rows(self) -> impl Iterator<Item = StagedTextRow> {
        self.messages.into_iter().map(|m| {
            let (id, role, phase, part_id, scope, text) = m.into_display_fields();
            StagedTextRow {
                id,
                role,
                phase,
                part_id,
                scope,
                text,
            }
        })
    }
}

//! Startup-only owner for the independently validated canonical conversation.
use super::super::{RemoteSessionStore, plain_history::PlainHistoryMessage};
use super::cache::{self, RemoteCacheSource, Section};
use crate::model_standard::CanonicalMessage;
use anyhow::Result;
use proteus_contracts::app_protocol::AppContextHistorySummary;

pub(super) enum StartupHistory {
    Full(Vec<CanonicalMessage>),
    Plain {
        rows: Vec<PlainHistoryMessage>,
        scratch: CanonicalMessage,
    },
}

impl StartupHistory {
    pub(super) fn prepare(history: Vec<CanonicalMessage>, try_plain: bool) -> Self {
        // This checks authoritative bytes independently of the hint about
        // staged display. Check everything before moving any owner.
        if !try_plain
            || !(4_096..=65_536).contains(&history.len())
            || !history.iter().all(PlainHistoryMessage::accepts)
        {
            return Self::Full(history);
        }
        let mut rows = Vec::new();
        if rows.try_reserve_exact(history.len()).is_err() {
            return Self::Full(history);
        }
        let mut donor = history.into_iter();
        let mut scratch = donor.next().expect("nonempty plain canonical history");
        rows.push(PlainHistoryMessage::take_from(&mut scratch));
        for mut message in donor {
            rows.push(PlainHistoryMessage::take_from(&mut message));
        }
        Self::Plain { rows, scratch }
    }

    pub(super) fn len(&self) -> usize {
        match self {
            Self::Full(rows) => rows.len(),
            Self::Plain { rows, .. } => rows.len(),
        }
    }

    pub(super) fn fingerprints(&mut self) -> Result<Vec<[u8; 32]>> {
        let hash = crate::core::history_message_sha256;
        match self {
            Self::Full(rows) => rows.iter().map(hash).collect(),
            Self::Plain { rows, scratch } => rows
                .iter_mut()
                .map(|row| row.with_typed_digest_row(scratch, hash))
                .collect(),
        }
    }

    pub(super) fn preview(&self) -> Option<String> {
        match self {
            Self::Full(rows) => cache::session_preview(rows),
            Self::Plain { rows, .. } => cache::plain_preview(rows),
        }
    }

    pub(super) fn context_summary(&self) -> AppContextHistorySummary {
        match self {
            Self::Full(rows) => crate::core::summarize_context_history(rows),
            Self::Plain { rows, .. } => {
                super::super::super::context_summary::summarize_plain_history(
                    rows.iter().map(PlainHistoryMessage::role_and_text),
                )
            }
        }
    }

    pub(super) fn write(
        &self,
        store: &RemoteSessionStore,
        source: &RemoteCacheSource,
    ) -> Option<Section> {
        match self {
            Self::Full(rows) => cache::write_transcript(store, source, rows),
            Self::Plain { rows, .. } => cache::write_plain_conversation(store, source, rows),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_standard::{ContentPart, MessageRole};

    fn fixture() -> Vec<CanonicalMessage> {
        (0..4_096)
            .map(|i| {
                let role = if i < 20 {
                    MessageRole::Assistant
                } else {
                    MessageRole::User
                };
                CanonicalMessage::text(role, format!("  {i} é🦀 \\ \n"))
            })
            .collect()
    }

    #[test]
    fn compact_preflight_keeps_same_text_owners_and_exact_summaries() {
        let rows = fixture();
        let ids: Vec<_> = rows
            .iter()
            .map(|row| {
                let ContentPart::Text { text } = &row.parts[0].payload else {
                    unreachable!();
                };
                text.as_ptr()
            })
            .collect();
        let preview = cache::session_preview(&rows);
        let context = crate::core::summarize_context_history(&rows);
        let ordered = rows
            .iter()
            .map(crate::core::history_message_sha256)
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let mut holder = StartupHistory::prepare(rows, true);
        assert_eq!(holder.len(), ids.len());
        assert_eq!(holder.preview(), preview);
        assert_eq!(holder.context_summary(), context);
        assert_eq!(holder.fingerprints().unwrap(), ordered);
        let StartupHistory::Plain { rows, .. } = holder else {
            panic!("fixture should use compact authoritative storage");
        };
        assert!(
            rows.iter()
                .zip(ids)
                .all(|(row, ptr)| row.role_and_text().1.as_ptr() == ptr)
        );
    }

    #[test]
    fn late_present_empty_name_does_not_convert_any_earlier_donor() {
        let mut rows = fixture();
        rows.last_mut().unwrap().name = Some(String::new());
        let original = rows.as_ptr();
        let StartupHistory::Full(rows) = StartupHistory::prepare(rows, true) else {
            panic!("present name is not canonical plain history");
        };
        assert_eq!(rows.as_ptr(), original);
        assert_eq!(rows.last().unwrap().name.as_deref(), Some(""));
    }
}

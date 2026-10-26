//! Keep unchanged payloads in place; replace only missing or unusable caches.
use super::*;
use crate::core::session_store::read_cache::{Updater, Writer};

enum Write {
    Update(Updater),
    Replace {
        writer: Writer,
        previous: Option<Reader>,
    },
}

pub(in crate::core::session_store::read_model) fn save(
    store: &SessionStore,
    source: &SourceFingerprint,
    model: &SessionReadModel,
) -> bool {
    let update = || {
        let writer = Updater::new(store.session_dir(), VIEW_CACHE, source)?;
        let header = writer.previous::<Header>("header")?;
        matches(store, &header).then_some(Write::Update(writer))
    };
    if update().and_then(|writer| write(writer, model)).is_some() {
        return true;
    }
    // Failed updates roll back before replacement tries the same cache lock.
    // This also heals a malformed cache and avoids waiting for a long reader.
    let replace = || {
        let writer = Writer::new(store.session_dir(), VIEW_CACHE, source)?;
        let previous = writer.previous(VIEW_CACHE).filter(|reader| {
            reader
                .one::<Header>("header")
                .is_some_and(|header| matches(store, &header))
        });
        write(Write::Replace { writer, previous }, model)
    };
    replace().is_some()
}

fn write(writer: Write, model: &SessionReadModel) -> Option<()> {
    let mut base =
        writer.revision().map(
            |revision| proteus_contracts::app_protocol::AppTranscriptBase {
                revision,
                common_prefix: 0,
            },
        );
    writer.one("usage", &model.usage)?;
    for (index, message) in model.conversation.messages.iter().enumerate() {
        writer.row("conversation", index, message)?;
    }
    for (index, message) in model.transcript.iter().enumerate() {
        let unchanged = writer.row("transcript", index, message)?;
        if let Some(base) = &mut base
            && base.common_prefix == index
            && unchanged
        {
            base.common_prefix += 1;
        }
    }
    for (index, observation) in model.conversation.model_context.iter().enumerate() {
        writer.row("context", index, observation)?;
    }
    writer.truncate("conversation", model.conversation.messages.len())?;
    writer.truncate("transcript", model.transcript.len())?;
    writer.truncate("context", model.conversation.model_context.len())?;
    writer.one(
        "header",
        &Header {
            summary: model.summary.clone(),
            revision: model.conversation.revision,
            conversation_len: model.conversation.messages.len(),
            transcript_len: model.transcript.len(),
            context_len: model.conversation.model_context.len(),
            context_summary: crate::core::summarize_context_history(&model.conversation.messages),
            base,
        },
    )?;
    writer.finish()
}

impl Write {
    fn revision(&self) -> Option<String> {
        match self {
            Self::Update(writer) => writer.revision(),
            Self::Replace { previous, .. } => previous.as_ref()?.revision(),
        }
    }

    fn one<T: Serialize>(&self, section: &str, data: &T) -> Option<()> {
        match self {
            Self::Update(writer) => writer.one(section, data),
            Self::Replace { writer, .. } => writer.one(section, data),
        }
    }

    fn row<T: Serialize>(&self, section: &str, index: usize, data: &T) -> Option<bool> {
        match self {
            Self::Update(writer) => writer.row(section, index, data),
            Self::Replace { writer, previous } => {
                let hash = writer.row(section, index, data)?;
                Some(
                    previous
                        .as_ref()
                        .and_then(|reader| reader.row_matches(section, index, &hash))
                        .unwrap_or(false),
                )
            }
        }
    }

    fn truncate(&self, section: &str, length: usize) -> Option<()> {
        match self {
            Self::Update(writer) => writer.truncate(section, length),
            Self::Replace { .. } => Some(()),
        }
    }

    fn finish(self) -> Option<()> {
        match self {
            Self::Update(writer) => writer.finish(),
            Self::Replace { writer, .. } => writer.finish(),
        }
    }
}

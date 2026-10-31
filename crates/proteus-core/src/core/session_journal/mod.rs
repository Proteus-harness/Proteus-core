mod context;
pub(crate) mod history_capture;
mod hooks;
pub(crate) use hooks::apply_recorded_hook_response;
mod projection;
mod recorder;
mod storage;
mod types;
mod usage;

pub(crate) use usage::UsageProjection;

pub use projection::JournalProjection;
pub(crate) use projection::JournalValidationState;
pub use recorder::{SessionExecutionRecorder, SessionToolExecutionRecorder};
pub use storage::{DEFAULT_BLOB_THRESHOLD_BYTES, JOURNAL_FILE};
pub use types::*;

pub(crate) use storage::{
    JournalReadCursor, JournalReadSnapshot, JournalRecordAttribution, JournalResumeState,
    JournalWriterState, SessionWriteOwnership, ValidatedJournalRead, WriterRetirement,
    append_record, initialize_writer_resume, initialize_writer_state, journal_path, load_records,
    load_validated_conversation, load_validated_history, load_validated_history_revision,
    load_validated_usage,
};
pub(crate) use storage::{history_message_sha256, redacted_history};

#[cfg(test)]
mod context_tests;
#[cfg(test)]
mod tests;

//! Serve verified startup rows while their disposable disk index is being written.
use super::{
    AppContextHistorySummary, AppTranscriptPage, DisplayRows, RemoteCacheSource,
    RemoteSessionStore, SessionUsageSnapshot, cache::PAGE_ROWS,
};
use std::sync::Arc;
use tokio::{
    runtime::Handle,
    sync::{Semaphore, SemaphorePermit},
};

// Only one large projection may outlive the startup call in this process.
// Readers of other sessions keep the ordinary synchronous publication path.
static SLOT: Semaphore = Semaphore::const_new(1);

pub(super) fn admit(
    rows: &DisplayRows,
    source: &RemoteCacheSource,
) -> Option<(Handle, SemaphorePermit<'static>, String)> {
    if rows.len() < 4_096 {
        return None;
    }
    Some((
        Handle::try_current().ok()?,
        SLOT.try_acquire().ok()?,
        source.revision()?,
    ))
}

pub(in super::super) struct PublishingTranscript {
    pub(super) revision: String,
    pub(super) rows: DisplayRows,
    pub(super) context_summary: AppContextHistorySummary,
    pub(super) usage: SessionUsageSnapshot,
    _permit: SemaphorePermit<'static>,
}

impl PublishingTranscript {
    pub(super) fn new(
        revision: String,
        rows: DisplayRows,
        context_summary: AppContextHistorySummary,
        usage: SessionUsageSnapshot,
        permit: SemaphorePermit<'static>,
    ) -> Self {
        Self {
            revision,
            rows,
            context_summary,
            usage,
            _permit: permit,
        }
    }

    pub(super) fn page(&self, before: Option<usize>) -> Option<AppTranscriptPage> {
        let total = self.rows.len();
        let end = before.unwrap_or(total);
        if end > total {
            return None;
        }
        let start = end.saturating_sub(PAGE_ROWS);
        Some(AppTranscriptPage {
            revision: self.revision.clone(),
            start,
            total,
            messages: self.rows.copy_range(start..end),
            base: None,
        })
    }
}

impl RemoteSessionStore {
    pub(super) fn current_publishing_transcript(&self) -> Option<Arc<PublishingTranscript>> {
        let pending = self.publishing_transcript(None)?;
        (pending.revision == RemoteCacheSource::read(self)?.revision()?).then_some(pending)
    }

    pub(super) fn publishing_transcript(
        &self,
        revision: Option<&str>,
    ) -> Option<Arc<PublishingTranscript>> {
        self.handles
            .publishing
            .lock()
            .unwrap()
            .as_ref()
            .filter(|pending| revision.is_none_or(|wanted| pending.revision == wanted))
            .cloned()
    }
}

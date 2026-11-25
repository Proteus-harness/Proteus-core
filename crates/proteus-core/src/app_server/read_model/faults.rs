//! Storage-boundary fault injection: display caches remain disposable across backends.
use crate::{
    contracts::{
        SessionStorage, SessionStorageRequest as Request, SessionStorageResponse as Response,
        StorageMetadataDocument,
    },
    core::{SessionStore, StorageService},
};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicUsize, Ordering},
};

#[derive(Clone, Default)]
pub(crate) struct CacheFaults {
    mode: Arc<AtomicU8>,
    journal_reads: Arc<AtomicUsize>,
}
impl CacheFaults {
    pub(crate) fn set(&self, name: &str) {
        self.mode.store(
            match name {
                "none" => 0,
                "missing" => 1,
                "malformed" => 2,
                "checksum" => 3,
                "unwritable" => 4,
                _ => panic!("unknown cache fault"),
            },
            Ordering::SeqCst,
        );
    }
    pub(crate) fn journal_reads(&self) -> usize {
        self.journal_reads.load(Ordering::SeqCst)
    }
    pub(crate) fn wrap(&self, inner: StorageService) -> StorageService {
        Arc::new(FaultStorage {
            inner,
            faults: self.clone(),
        })
    }
    pub(crate) fn store(&self, mut store: SessionStore) -> SessionStore {
        store.service = self.wrap(store.service.clone());
        store
    }
}
struct FaultStorage {
    inner: StorageService,
    faults: CacheFaults,
}
impl FaultStorage {
    fn intercept(&self, request: &Request) -> Option<Response> {
        if matches!(
            request,
            Request::OpenSnapshot { .. }
                | Request::OpenDisplaySnapshot { .. }
                | Request::ReadRecords { .. }
                | Request::ReadDisplayRecords { .. }
        ) {
            self.faults.journal_reads.fetch_add(1, Ordering::SeqCst);
        }
        match (self.faults.mode.load(Ordering::SeqCst), request) {
            (1 | 4, Request::ReadCache { .. }) => Some(Response::Metadata { document: None }),
            (2, Request::ReadCache { .. }) => Some(Response::Metadata {
                document: Some(StorageMetadataDocument {
                    etag: "fault".into(),
                    value: serde_json::json!({"invalid":"cache document"}),
                }),
            }),
            (3, Request::ReadCache { key, .. }) if key.starts_with("display-data-") => {
                Some(Response::Metadata {
                    document: Some(StorageMetadataDocument {
                        etag: "fault".into(),
                        value: serde_json::json!(["wrong checksum"]),
                    }),
                })
            }
            (4, Request::WriteCache { .. }) => Some(Response::CacheWritten { etag: None }),
            _ => None,
        }
    }
}
#[async_trait]
impl SessionStorage for FaultStorage {
    async fn execute(&self, request: Request) -> Result<Response> {
        match self.intercept(&request) {
            Some(response) => Ok(response),
            None => self.inner.execute(request).await,
        }
    }
    fn execute_blocking(&self, request: Request) -> Result<Response> {
        match self.intercept(&request) {
            Some(response) => Ok(response),
            None => self.inner.execute_blocking(request),
        }
    }
}

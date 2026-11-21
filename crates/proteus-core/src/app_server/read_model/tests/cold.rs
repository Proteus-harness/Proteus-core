//! A cold display can be validated even when an ordinary source probe cannot finish.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{
    contracts::{
        SessionStorage, SessionStorageFailure, SessionStorageRequest as Request,
        SessionStorageResponse as Response,
    },
    core::StorageService,
};

struct ColdStorage {
    inner: StorageService,
    projected: AtomicBool,
    missing_after_projection: bool,
}

#[async_trait::async_trait]
impl SessionStorage for ColdStorage {
    fn supports_display_projection(&self) -> bool {
        true
    }

    async fn execute(&self, request: Request) -> anyhow::Result<Response> {
        self.execute_blocking(request)
    }

    fn execute_blocking(&self, request: Request) -> anyhow::Result<Response> {
        match &request {
            Request::OpenDisplaySnapshot { .. } => {
                let response = self.inner.execute_blocking(request)?;
                self.projected.store(true, Ordering::SeqCst);
                Ok(response)
            }
            Request::OpenSnapshot { .. } if !self.projected.load(Ordering::SeqCst) => {
                Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into())
            }
            Request::OpenSnapshot { .. } if self.missing_after_projection => {
                Err(SessionStorageFailure::MissingSession.into())
            }
            _ => self.inner.execute_blocking(request),
        }
    }
}

#[tokio::test]
async fn cold_latest_page_uses_the_display_source_and_checks_it_again_before_returning() {
    let (_root, _workspace, store, expected, faults) = super::fixture(51).await;
    faults.set("missing");
    for missing_after_projection in [false, true] {
        let mut cold = store.clone();
        cold.service = Arc::new(ColdStorage {
            inner: store.service.clone(),
            projected: false.into(),
            missing_after_projection,
        });
        let result = super::transcript_page(&cold, None, None);
        if missing_after_projection {
            assert_eq!(
                result.unwrap_err().downcast_ref::<SessionStorageFailure>(),
                Some(&SessionStorageFailure::MissingSession)
            );
        } else {
            let page = result.unwrap();
            assert_eq!(page.revision, expected.revision);
            assert_eq!((page.start, page.total), (1, 51));
            assert_eq!(
                serde_json::to_value(page.messages).unwrap(),
                serde_json::to_value(&expected.messages).unwrap()
            );
        }
    }
}

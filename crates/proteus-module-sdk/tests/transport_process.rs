//! A real SDK child, with no libtest output on its protocol stdout.
use std::{sync::Mutex, time::Duration};

use anyhow::{Result, bail, ensure};
use proteus_module_sdk::{
    Export,
    contracts::contracts::{
        ProcessComponentExportRef, SessionLocator, SessionStorageRequest, SessionStorageResponse,
        StorageMetadataDocument, StorageRevision, StorageSnapshot, storage_value_etag,
    },
    protocol::{
        ProcessComponentBinding, ProcessExportBinding,
        v3::{
            COMPONENT_MAX_FRAME_BYTES_ENV, ComponentBroker, ComponentBrokerOptions,
            ComponentTransportConfig, InvocationTerminal,
        },
    },
    session_storage::{SessionStorageExport, SessionStorageModule},
};
use proteus_process_host::ProcessSpec;
use serde_json::json;

const CHILD_ENV: &str = "PROTEUS_SDK_TRANSPORT_TEST_CHILD";
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Default)]
struct EchoCache(Mutex<Option<StorageMetadataDocument>>);
impl SessionStorageModule for EchoCache {
    fn execute(&self, request: SessionStorageRequest) -> Result<SessionStorageResponse> {
        match request {
            SessionStorageRequest::WriteCache { value, .. } => {
                let etag = storage_value_etag(&value)?;
                *self.0.lock().unwrap() = Some(StorageMetadataDocument {
                    etag: etag.clone(),
                    value,
                });
                Ok(SessionStorageResponse::CacheWritten { etag: Some(etag) })
            }
            SessionStorageRequest::ReadCache { .. } => Ok(SessionStorageResponse::Metadata {
                document: self.0.lock().unwrap().clone(),
            }),
            _ => bail!("unexpected transport probe operation"),
        }
    }
}

fn main() -> Result<()> {
    if std::env::var_os(CHILD_ENV).is_some() {
        return proteus_module_sdk::run(|binding| {
            Ok(
                Box::new(SessionStorageExport::new(binding, EchoCache::default())?)
                    as Box<dyn Export>,
            )
        });
    }
    round_trip(16 * 1024 * 1024, 16 * 1024 * 1024, 9 * 1024 * 1024)?;
    round_trip(8 * 1024 * 1024, 8 * 1024 * 1024, 64)?;
    rejected(64 * 1024, 64 * 1024, 128 * 1024)?;
    // The host permits this request, but the child must enforce its own bound.
    rejected(16 * 1024 * 1024, 8 * 1024 * 1024, 9 * 1024 * 1024)?;
    Ok(())
}

fn request(bytes: usize) -> SessionStorageRequest {
    let session = SessionLocator {
        session_id: proteus_module_sdk::contracts::domain::new_session_id(),
        key: "transport-probe".into(),
    };
    let source = StorageSnapshot {
        session: session.clone(),
        snapshot_id: "fixed-prefix".into(),
        revision: StorageRevision::default(),
        content_sha256: "a".repeat(64),
    };
    SessionStorageRequest::WriteCache {
        session,
        key: "large-value".into(),
        source,
        value: json!({"data":"x".repeat(bytes)}),
    }
}

fn broker(host_limit: usize, child_limit: usize) -> Result<ComponentBroker> {
    let spec = ProcessSpec::new(std::env::current_exe()?.to_string_lossy())
        .env(CHILD_ENV, "1")
        .env(COMPONENT_MAX_FRAME_BYTES_ENV, child_limit.to_string());
    let binding = ProcessComponentBinding::new(
        "transport-probe",
        [ProcessExportBinding::new(
            "session_storage",
            "echo",
            "v1",
            json!({}),
        )?],
    )?;
    let options = ComponentTransportConfig {
        max_frame_bytes: host_limit,
    }
    .apply(ComponentBrokerOptions::default())?;
    ComponentBroker::connect(spec, binding, options)
}

fn invoke(broker: &ComponentBroker, request: &SessionStorageRequest) -> Result<InvocationTerminal> {
    Ok(broker.invoke_blocking(
        &ProcessComponentExportRef::new("session_storage", "echo"),
        "execute",
        serde_json::to_value(request)?,
        TIMEOUT,
    )?)
}

fn round_trip(host_limit: usize, child_limit: usize, bytes: usize) -> Result<()> {
    let broker = broker(host_limit, child_limit)?;
    let request = request(bytes);
    let SessionStorageRequest::WriteCache {
        session,
        key,
        source,
        ..
    } = &request
    else {
        unreachable!()
    };
    let read = SessionStorageRequest::ReadCache {
        session: session.clone(),
        key: key.clone(),
        source: Some(source.clone()),
    };
    let terminal = invoke(&broker, &request)?;
    let InvocationTerminal::Success(value) = terminal else {
        bail!("transport probe failed: {terminal:?}")
    };
    let response: SessionStorageResponse = serde_json::from_value(value)?;
    request.validate_response(&response)?;
    ensure!(
        matches!(
            response,
            SessionStorageResponse::CacheWritten { etag: Some(_) }
        ),
        "missing compact write receipt"
    );
    drop(request);
    let InvocationTerminal::Success(value) = invoke(&broker, &read)? else {
        bail!("cache read failed")
    };
    let response: SessionStorageResponse = serde_json::from_value(value)?;
    read.validate_response(&response)?;
    let SessionStorageResponse::Metadata {
        document: Some(document),
    } = response
    else {
        bail!("missing echo response")
    };
    ensure!(document.value["data"].as_str().map(str::len) == Some(bytes));
    Ok(())
}

fn rejected(host_limit: usize, child_limit: usize, bytes: usize) -> Result<()> {
    let broker = broker(host_limit, child_limit)?;
    let result = invoke(&broker, &request(bytes));
    ensure!(
        !matches!(result, Ok(InvocationTerminal::Success(_))),
        "oversized frame was accepted"
    );
    Ok(())
}

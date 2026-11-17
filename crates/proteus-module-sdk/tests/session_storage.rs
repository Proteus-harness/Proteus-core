mod support;

use std::{
    io::{BufRead, BufReader, Write},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

use anyhow::{Result, bail};
use proteus_module_sdk::{
    Export,
    contracts::contracts::{
        ProcessComponentExportInitialize, ProcessComponentInitialize, ProcessModuleComposition,
        SessionStorageRequest, SessionStorageResponse,
    },
    session_storage::{SessionStorageExport, SessionStorageModule},
};
use serde_json::{Value, json};

struct Probe(Arc<AtomicUsize>);

impl SessionStorageModule for Probe {
    fn execute(&self, request: SessionStorageRequest) -> Result<SessionStorageResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        match request {
            SessionStorageRequest::List { .. } => {
                Ok(SessionStorageResponse::Sessions { sessions: vec![] })
            }
            _ => bail!("unexpected probe operation"),
        }
    }
}

fn binding() -> ProcessComponentExportInitialize {
    ProcessComponentExportInitialize::new(
        "session_storage",
        "probe",
        "v1",
        ProcessModuleComposition::SelectOne,
        json!({}),
        Vec::<String>::new(),
    )
}

fn send(writer: &mut impl Write, frame: Value) {
    serde_json::to_writer(&mut *writer, &frame).unwrap();
    writer.write_all(b"\n").unwrap();
    writer.flush().unwrap();
}

fn read(reader: &mut impl BufRead) -> Value {
    let mut line = String::new();
    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
    serde_json::from_str(&line).unwrap()
}

#[test]
fn storage_export_crosses_sdk_frames_without_callbacks_and_rejects_invalid_requests() {
    let (worker_reader, mut writer) = support::stream();
    let (reader, worker_writer) = support::stream();
    let calls = Arc::new(AtomicUsize::new(0));
    let backend_calls = calls.clone();
    let worker = thread::spawn(move || {
        proteus_module_sdk::serve(
            BufReader::new(worker_reader),
            worker_writer,
            move |binding| {
                Ok(Box::new(SessionStorageExport::new(
                    binding,
                    Probe(backend_calls.clone()),
                )?) as Box<dyn Export>)
            },
        )
    });
    let mut reader = BufReader::new(reader.with_timeout());
    send(
        &mut writer,
        json!({"jsonrpc":"2.0", "id":"h:9:0", "method":"initialize",
        "params":ProcessComponentInitialize::new("storage-test", [binding()])}),
    );
    let manifest = read(&mut reader);
    assert_eq!(manifest["result"]["exports"][0]["slot"], "session_storage");
    assert_eq!(
        manifest["result"]["exports"][0]["composition"],
        "select_one"
    );
    for (sequence, method, params, success) in [
        (
            1,
            "execute",
            json!({"operation":"list", "config_root":"/unused", "workspace_path":null, "audit":false}),
            true,
        ),
        (
            2,
            "execute",
            json!({"operation":"read_metadata", "session":{"session_id":"00000000-0000-0000-0000-000000000001", "key":"fixture"}, "key":"../outside"}),
            false,
        ),
        (3, "describe", Value::Null, false),
    ] {
        let id = format!("h:9:{sequence}");
        send(
            &mut writer,
            json!({"jsonrpc":"2.0", "id":id, "method":method,
            "params":{"export":{"slot":"session_storage", "module_id":"probe"},
                "lineage":{"root_invocation_id":id,"parent_invocation_id":null,"depth":0}, "params":params}}),
        );
        let response = read(&mut reader);
        assert_eq!(
            response["id"], id,
            "storage export must not emit a host callback"
        );
        assert_eq!(response.get("result").is_some(), success, "{response}");
        assert_eq!(response.get("error").is_none(), success, "{response}");
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "invalid input must not reach storage"
    );
    drop(writer);
    worker.join().unwrap().unwrap();
}

#[test]
fn storage_export_rejects_wrong_slot_version_and_composition() {
    for (slot, version, composition) in [
        ("tool", "v1", ProcessModuleComposition::SelectOne),
        ("session_storage", "v0", ProcessModuleComposition::SelectOne),
        (
            "session_storage",
            "v1",
            ProcessModuleComposition::OrderedMany,
        ),
    ] {
        let mut requested = binding();
        requested.slot = slot.into();
        requested.contract_version = version.into();
        requested.composition = composition;
        assert!(
            SessionStorageExport::new(requested, Probe(Arc::new(AtomicUsize::new(0)))).is_err()
        );
    }
}

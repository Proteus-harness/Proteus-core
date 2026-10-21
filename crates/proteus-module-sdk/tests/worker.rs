mod support;

use std::{
    io::{BufRead, BufReader, Write},
    thread,
    time::Duration,
};

use anyhow::{Result, bail};
use proteus_module_sdk::{
    Export,
    contracts::{
        contracts::{
            ExecutionAttribution, ProcessComponentExportInitialize, ProcessComponentInitialize,
            ProcessInvocationLineage, ProcessModuleCancel, ProcessModuleCancelCause,
            ProcessModuleComposition, ProcessToolDefinition, ProcessToolInvokeInput,
        },
        domain::{
            SkillRuntimeSettings, ToolCall, ToolResult, ToolSafety, ToolSpec, new_execution_id,
        },
        process_module::ToolModuleHost,
    },
    tool::{ToolExport, ToolModule},
};
use serde_json::{Value, json};

struct Probe;

impl ToolModule for Probe {
    fn definitions(&self) -> Vec<ProcessToolDefinition> {
        vec![ProcessToolDefinition {
            spec: ToolSpec::new(
                "probe",
                "SDK worker test tool",
                json!({"type":"object", "required":["mode"], "properties":{
                    "mode":{"type":"string"}
                }, "additionalProperties":false}),
                ToolSafety::ReadOnly,
            ),
            model_visible: true,
            user_command: None,
        }]
    }

    fn invoke(
        &self,
        input: ProcessToolInvokeInput,
        host: &mut dyn ToolModuleHost,
    ) -> Result<ToolResult> {
        let output = match input.call.args["mode"].as_str().unwrap() {
            "error" => bail!("synthetic module failure"),
            "domain_error" => return Ok(ToolResult::error(input.call.id, "ordinary tool failure")),
            "callback" => host.conversation_snapshot_json()?,
            "nested" => host.execute_tool_json(serde_json::to_string(&ToolCall::new(
                "child-tool-call",
                "probe",
                json!({"mode":"ok"}),
            ))?)?,
            "wait" => {
                // Synchronize the host before waiting cooperatively; no sleeps in
                // the test can substitute for proof the invocation actually began.
                host.conversation_snapshot_json()?;
                while !host.is_cancelled()? {
                    thread::sleep(Duration::from_millis(1));
                }
                "cancellation observed".into()
            }
            mode => mode.to_owned(),
        };
        Ok(ToolResult::ok(input.call.id, output))
    }
}

fn binding() -> ProcessComponentExportInitialize {
    ProcessComponentExportInitialize::new(
        "tool",
        "probe",
        "v6",
        ProcessModuleComposition::OrderedMany,
        json!({}),
        Vec::<String>::new(),
    )
}

fn factory(binding: ProcessComponentExportInitialize) -> Result<Box<dyn Export>> {
    if binding.module_id != "probe" || binding.module_config != json!({}) {
        bail!("unsupported export identity or config");
    }
    Ok(Box::new(ToolExport::new(binding, Probe)?))
}

struct Worker {
    writer: Option<support::Writer>,
    reader: BufReader<support::Reader>,
    worker: Option<thread::JoinHandle<Result<()>>>,
}

impl Worker {
    fn new() -> Self {
        let (worker_reader, host_writer) = support::stream();
        let (host_reader, worker_writer) = support::stream();
        let task = thread::spawn(move || {
            proteus_module_sdk::serve(BufReader::new(worker_reader), worker_writer, factory)
        });
        Self {
            reader: BufReader::new(host_reader.with_timeout()),
            writer: Some(host_writer),
            worker: Some(task),
        }
    }

    fn send(&mut self, frame: Value) {
        serde_json::to_writer(self.writer.as_mut().unwrap(), &frame).unwrap();
        self.writer.as_mut().unwrap().write_all(b"\n").unwrap();
        self.writer.as_mut().unwrap().flush().unwrap();
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        assert_ne!(
            self.reader.read_line(&mut line).unwrap(),
            0,
            "worker closed stdout"
        );
        serde_json::from_str(&line).unwrap()
    }

    fn initialize(&mut self) {
        self.send(json!({"jsonrpc":"2.0", "id":"h:7:0", "method":"initialize",
            "params":ProcessComponentInitialize::new("sdk-test", [binding()])}));
        let response = self.read();
        assert_eq!(response["id"], "h:7:0");
        assert_eq!(response["result"]["protocol_version"], "v3");
        assert_eq!(response["result"]["exports"][0]["module_id"], "probe");
    }

    fn invoke(&mut self, id: &str, mode: &str) {
        self.invoke_with_lineage(id, mode, ProcessInvocationLineage::root(id));
    }

    fn invoke_with_lineage(&mut self, id: &str, mode: &str, lineage: ProcessInvocationLineage) {
        let input = ProcessToolInvokeInput {
            call: ToolCall::new(format!("tool-{id}"), "probe", json!({"mode":mode})),
            cwd: std::env::current_dir().unwrap(),
            attribution: ExecutionAttribution::detached(new_execution_id()),
            skills: SkillRuntimeSettings::default(),
        };
        self.send(
            json!({"jsonrpc":"2.0", "id":id, "method":"invoke", "params":{
                "export":{"slot":"tool", "module_id":"probe"}, "lineage":lineage, "params":input,
            }}),
        );
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.writer.take();
        if let Some(task) = self.worker.take() {
            let outcome = task.join();
            if !thread::panicking() {
                outcome.unwrap().unwrap();
            }
        }
    }
}

#[test]
fn actual_wire_handshake_list_and_detached_tool_invocation() {
    let mut worker = Worker::new();
    worker.initialize();
    worker.send(
        json!({"jsonrpc":"2.0", "id":"h:7:1", "method":"list", "params":{
            "export":{"slot":"tool","module_id":"probe"},
            "lineage":ProcessInvocationLineage::root("h:7:1"), "params":null,
        }}),
    );
    let list = worker.read();
    assert_eq!(list["result"]["result"][0]["spec"]["name"], "probe");
    worker.invoke("h:7:2", "ok");
    let response = worker.read();
    assert_eq!(response["result"]["result"]["call_id"], "tool-h:7:2");
    assert_eq!(response["result"]["result"]["output"], "ok");
}

#[test]
fn module_error_is_terminal_but_does_not_destroy_component() {
    let mut worker = Worker::new();
    worker.initialize();
    worker.invoke("h:7:1", "error");
    let failed = worker.read();
    assert_eq!(failed["error"]["code"], -32000);
    assert!(
        failed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("synthetic module failure")
    );
    worker.invoke("h:7:2", "domain_error");
    let failed_tool = worker.read();
    assert_eq!(failed_tool["result"]["result"]["ok"], false);
    assert_eq!(
        failed_tool["result"]["result"]["error"],
        "ordinary tool failure"
    );
    worker.invoke("h:7:3", "ok");
    assert_eq!(worker.read()["result"]["result"]["ok"], true);
}

#[test]
fn callback_has_invocation_identity_and_nested_dispatch_is_reentrant() {
    let mut worker = Worker::new();
    worker.initialize();
    worker.invoke("h:7:1", "nested");
    let callback = worker.read();
    assert_eq!(
        callback["method"],
        proteus_module_sdk::contracts::contracts::OPERATION_HOST_EXECUTE_TOOL_METHOD
    );
    assert_eq!(callback["params"]["invocation_id"], "h:7:1");
    assert!(callback["id"].as_str().unwrap().starts_with("m:7:"));
    worker.invoke_with_lineage(
        "h:7:2",
        "ok",
        ProcessInvocationLineage {
            root_invocation_id: "h:7:1".into(),
            parent_invocation_id: Some("h:7:1".into()),
            depth: 1,
        },
    );
    let nested = worker.read();
    assert_eq!(nested["id"], "h:7:2");
    assert_eq!(nested["result"]["result"]["output"], "ok");
    worker.send(json!({"jsonrpc":"2.0", "id":callback["id"], "result":{"output":"child result"}}));
    let parent = worker.read();
    assert_eq!(parent["id"], "h:7:1");
    assert_eq!(
        parent["result"]["result"]["output"],
        "{\"output\":\"child result\"}"
    );
}

#[test]
fn targeted_cancel_preserves_other_invocation_and_component() {
    let mut worker = Worker::new();
    worker.initialize();
    worker.invoke("h:7:1", "wait");
    let waiting = worker.read();
    assert_eq!(waiting["params"]["invocation_id"], "h:7:1");
    worker.send(json!({"jsonrpc":"2.0", "id":waiting["id"], "result":null}));
    worker.invoke("h:7:2", "callback");
    let other = worker.read();
    assert_eq!(other["params"]["invocation_id"], "h:7:2");
    worker.send(
        json!({"jsonrpc":"2.0", "method":"$/cancelRequest", "params":ProcessModuleCancel::new("h:7:1", ProcessModuleCancelCause::User)}),
    );
    worker.send(json!({"jsonrpc":"2.0", "id":other["id"], "result":{"messages":[]}}));
    let mut results = [worker.read(), worker.read()];
    results.sort_by_key(|response| response["id"].as_str().unwrap().to_owned());
    assert_eq!(results[0]["id"], "h:7:1");
    assert_eq!(results[0]["error"]["code"], -32800);
    assert_eq!(results[1]["id"], "h:7:2");
    assert_eq!(
        results[1]["result"]["result"]["output"],
        "{\"messages\":[]}"
    );
    worker.invoke("h:7:3", "ok");
    assert_eq!(worker.read()["result"]["result"]["ok"], true);
}

#[test]
fn old_wire_version_and_unrecognized_exports_fail_closed() {
    for (version, module_id, config) in [
        ("v2", "probe", json!({})),
        ("v3", "not-provided", json!({})),
        ("v3", "probe", json!({"unknown": true})),
    ] {
        let mut requested = binding();
        requested.module_id = module_id.into();
        requested.module_config = config;
        let mut initialize = ProcessComponentInitialize::new("sdk-test", [requested]);
        initialize.protocol_version = version.into();
        let line = format!(
            "{}\n",
            json!({"jsonrpc":"2.0", "id":"h:1:0", "method":"initialize", "params":initialize})
        );
        proteus_module_sdk::serve(std::io::Cursor::new(line), std::io::sink(), factory)
            .expect_err("unsupported handshake must fail");
    }
}

#[test]
fn invalid_input_and_cross_slot_method_never_reach_handler() {
    let mut worker = Worker::new();
    worker.initialize();
    for (id, method, params) in [
        ("h:7:1", "stream", Value::Null),
        ("h:7:2", "list", json!({})),
    ] {
        worker.send(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":{
            "export":{"slot":"tool","module_id":"probe"},
            "lineage":ProcessInvocationLineage::root(id), "params":params,
        }}));
        assert_eq!(worker.read()["error"]["code"], -32000);
    }
}

#[test]
fn raw_arguments_are_validated_and_executed_as_the_same_value() {
    let mut worker = Worker::new();
    worker.initialize();
    let input = ProcessToolInvokeInput {
        call: ToolCall::new("raw-call", "probe", json!({}))
            .with_raw_arguments(r#"{"mode":"from-raw"}"#),
        cwd: std::env::current_dir().unwrap(),
        attribution: ExecutionAttribution::detached(new_execution_id()),
        skills: SkillRuntimeSettings::default(),
    };
    worker.send(
        json!({"jsonrpc":"2.0", "id":"h:7:1", "method":"invoke", "params":{
            "export":{"slot":"tool", "module_id":"probe"},
            "lineage":ProcessInvocationLineage::root("h:7:1"), "params":input,
        }}),
    );
    assert_eq!(worker.read()["result"]["result"]["output"], "from-raw");
}

struct ClosingProbe(std::sync::mpsc::Sender<(bool, bool)>);

impl ToolModule for ClosingProbe {
    fn definitions(&self) -> Vec<ProcessToolDefinition> {
        Probe.definitions()
    }

    fn invoke(
        &self,
        input: ProcessToolInvokeInput,
        host: &mut dyn ToolModuleHost,
    ) -> Result<ToolResult> {
        assert!(
            host.conversation_snapshot_json().is_err(),
            "pending callback must wake on shutdown"
        );
        let canceled = host.is_cancelled()?;
        // A callback begun after shutdown must fail immediately as well. It
        // cannot enter the drained map and wait for a reader that has returned.
        let subsequent_callback_failed = host.conversation_snapshot_json().is_err();
        self.0.send((canceled, subsequent_callback_failed))?;
        Ok(ToolResult::ok(input.call.id, "closed"))
    }
}

#[test]
fn eof_and_parse_failure_cancel_active_work_and_release_callback_waiters() {
    for malformed in [false, true] {
        let (worker_reader, mut host_writer) = support::stream();
        let (host_reader, worker_writer) = support::stream();
        let (done_sender, done_receiver) = std::sync::mpsc::channel();
        let task = thread::spawn(move || {
            proteus_module_sdk::serve(
                BufReader::new(worker_reader),
                worker_writer,
                move |binding| {
                    Ok(Box::new(ToolExport::new(
                        binding,
                        ClosingProbe(done_sender.clone()),
                    )?))
                },
            )
        });
        let mut reader = BufReader::new(host_reader.with_timeout());
        let initialize = json!({"jsonrpc":"2.0", "id":"h:7:0", "method":"initialize",
            "params":ProcessComponentInitialize::new("shutdown-test", [binding()])});
        writeln!(host_writer, "{initialize}").unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(
            serde_json::from_str::<Value>(&line)
                .unwrap()
                .get("result")
                .is_some()
        );
        let input = ProcessToolInvokeInput {
            call: ToolCall::new("closing-call", "probe", json!({"mode":"callback"})),
            cwd: std::env::current_dir().unwrap(),
            attribution: ExecutionAttribution::detached(new_execution_id()),
            skills: SkillRuntimeSettings::default(),
        };
        let invoke = json!({"jsonrpc":"2.0", "id":"h:7:1", "method":"invoke", "params":{
            "export":{"slot":"tool","module_id":"probe"},
            "lineage":ProcessInvocationLineage::root("h:7:1"), "params":input,
        }});
        writeln!(host_writer, "{invoke}").unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["params"]["invocation_id"],
            "h:7:1"
        );
        if malformed {
            writeln!(host_writer, "{{broken-json").unwrap();
        }
        drop(host_writer);
        let outcome = task.join().unwrap();
        assert_eq!(outcome.is_err(), malformed);
        assert_eq!(
            done_receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            (true, true)
        );
    }
}

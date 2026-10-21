use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use anyhow::{Context, Result, anyhow, bail};
use proteus_contracts::contracts::{
    PROCESS_COMPONENT_INITIALIZE_METHOD, PROCESS_COMPONENT_PROTOCOL_VERSION,
    PROCESS_MODULE_CANCEL_METHOD, PROCESS_MODULE_CANCELLED_CODE, ProcessComponentExportInitialize,
    ProcessComponentInitialize, ProcessComponentInvocation, ProcessComponentManifest,
    ProcessInvocationLineage, ProcessModuleCancel,
};
use proteus_module_protocol::{
    ProcessModuleRpcError, process_contract_authority,
    v3::{ComponentFrame, WireDirection, parse_component_frame, parse_wire_id},
};
use serde_json::Value;

use crate::{
    Export,
    hosts::HostBridge,
    transport::{FrameReader, ModuleTransport, rpc_error, rpc_success},
};

const MAX_ACTIVE_INVOCATIONS: usize = 32;

struct ModuleExports {
    component_id: String,
    exports: BTreeMap<(String, String), Box<dyn Export>>,
}

#[derive(Clone)]
struct ActiveInvocation {
    cancellation: Arc<AtomicBool>,
    lineage: ProcessInvocationLineage,
}

struct ModuleRuntime {
    component: ModuleExports,
    transport: Arc<ModuleTransport>,
    generation: u64,
    active: Mutex<HashMap<String, ActiveInvocation>>,
}

/// Serve newline-delimited JSON-RPC on stdin/stdout. Logs belong on stderr.
///
/// The factory only loads requested exports. It must reject unsupported identities
/// and configuration; the SDK never chooses implementations or defaults.
pub fn run(
    factory: impl Fn(ProcessComponentExportInitialize) -> Result<Box<dyn Export>>,
) -> Result<()> {
    serve(
        std::io::BufReader::new(std::io::stdin()),
        std::io::stdout(),
        factory,
    )
}

/// Serve the same protocol over owned streams, useful for worker integration tests.
/// stdin must have a single reader; invocation threads share the synchronized writer.
pub fn serve(
    reader: impl std::io::BufRead + 'static,
    writer: impl std::io::Write + Send + 'static,
    factory: impl Fn(ProcessComponentExportInitialize) -> Result<Box<dyn Export>>,
) -> Result<()> {
    let mut reader = FrameReader::new(reader);
    let initialize_value = reader
        .read()?
        .ok_or_else(|| anyhow!("missing initialize request"))?;
    let ComponentFrame::Request {
        id: initialize_id,
        method,
        params,
    } = parse_component_frame(initialize_value)?
    else {
        bail!("first frame must be a JSON-RPC initialize request");
    };
    if method != PROCESS_COMPONENT_INITIALIZE_METHOD {
        bail!("first request must be JSON-RPC initialize");
    }
    let initialize_wire_id = parse_wire_id(&initialize_id)?;
    if initialize_wire_id.direction != WireDirection::Host || initialize_wire_id.sequence != 0 {
        bail!("initialize request must use host id sequence zero");
    }
    let binding: ProcessComponentInitialize =
        serde_json::from_value(params).context("invalid process component initialize params")?;
    validate_initialize(&binding)?;

    let mut exports = BTreeMap::new();
    for export in binding.exports {
        let key = (export.slot.clone(), export.module_id.clone());
        let implementation = factory(export.clone())?;
        validate_export_manifest(&export, &implementation.manifest())?;
        exports.insert(key, implementation);
    }
    let manifest = ProcessComponentManifest {
        protocol_version: PROCESS_COMPONENT_PROTOCOL_VERSION.to_owned(),
        component_id: binding.component_id.clone(),
        exports: exports.values().map(|export| export.manifest()).collect(),
        metadata: Default::default(),
    };
    let transport = Arc::new(ModuleTransport::new(initialize_wire_id.generation, writer));
    transport.write(&rpc_success(
        &initialize_id,
        serde_json::to_value(manifest)?,
    ))?;

    let runtime = Arc::new(ModuleRuntime {
        component: ModuleExports {
            component_id: binding.component_id,
            exports,
        },
        transport,
        generation: initialize_wire_id.generation,
        active: Mutex::new(HashMap::new()),
    });

    // EOF and every `?` failure must release cooperative work and callbacks,
    // including when this library is embedded rather than the process exiting.
    let _shutdown = RuntimeShutdown(Arc::clone(&runtime));
    while let Some(value) = reader.read()? {
        match parse_component_frame(value)? {
            ComponentFrame::Request { id, method, params } => {
                runtime.start_invocation(id, method, params)?;
            }
            ComponentFrame::Notification { method, params } => runtime.cancel(method, params)?,
            ComponentFrame::Response { id, result } => {
                runtime.complete_callback(id, result)?;
            }
        }
    }
    Ok(())
}

struct RuntimeShutdown(Arc<ModuleRuntime>);

impl Drop for RuntimeShutdown {
    fn drop(&mut self) {
        if let Ok(active) = self.0.active.lock() {
            for invocation in active.values() {
                invocation.cancellation.store(true, Ordering::SeqCst);
            }
        }
        self.0.transport.close();
    }
}

impl ModuleRuntime {
    fn start_invocation(
        self: &Arc<Self>,
        request_id: String,
        method: String,
        params: Value,
    ) -> Result<()> {
        if method == PROCESS_COMPONENT_INITIALIZE_METHOD {
            bail!("invalid component invocation method {method:?}");
        }
        self.validate_invocation_id(&request_id)?;
        let call: ProcessComponentInvocation = serde_json::from_value(params)
            .context("invalid process component invocation params")?;
        let cancellation = Arc::new(AtomicBool::new(false));
        {
            let mut active = self
                .active
                .lock()
                .map_err(|_| anyhow!("active invocation map poisoned"))?;
            if active.contains_key(&request_id) {
                bail!("invocation id was reused: {request_id}");
            }
            validate_lineage(&request_id, &call.lineage, &active, self.generation)?;
            if active.len() >= MAX_ACTIVE_INVOCATIONS {
                drop(active);
                self.transport.write(&rpc_error(
                    &request_id,
                    ProcessModuleRpcError::new(
                        -32014,
                        "component active invocation capacity is exhausted",
                    ),
                ))?;
                return Ok(());
            }
            active.insert(
                request_id.clone(),
                ActiveInvocation {
                    cancellation: Arc::clone(&cancellation),
                    lineage: call.lineage.clone(),
                },
            );
        }

        let runtime = Arc::clone(self);
        let invocation_id = request_id;
        let thread_id = invocation_id.replace(':', "-");
        let invocation_id_for_thread = invocation_id.clone();
        match thread::Builder::new()
            .name(format!("component-invocation-{thread_id}"))
            .spawn(move || {
                runtime.execute_invocation(invocation_id_for_thread, method, call, cancellation);
            }) {
            Ok(handle) => drop(handle),
            Err(error) => {
                let response = rpc_error(
                    &invocation_id,
                    ProcessModuleRpcError::new(
                        -32015,
                        format!("failed to start invocation task: {error}"),
                    ),
                );
                self.transport.write(&response)?;
                self.finish_invocation(&invocation_id)?;
            }
        }
        Ok(())
    }

    fn execute_invocation(
        self: Arc<Self>,
        invocation_id: String,
        method: String,
        call: ProcessComponentInvocation,
        cancellation: Arc<AtomicBool>,
    ) {
        let bridge = HostBridge::new(
            Arc::clone(&self.transport),
            invocation_id.clone(),
            Arc::clone(&cancellation),
        );
        let result = self.component.dispatch(call, &method, &bridge);
        let response = if cancellation.load(Ordering::SeqCst) {
            rpc_error(
                &invocation_id,
                ProcessModuleRpcError::new(PROCESS_MODULE_CANCELLED_CODE, "canceled"),
            )
        } else {
            match result {
                Ok(result) => rpc_success(&invocation_id, result),
                Err(error) => rpc_error(&invocation_id, terminal_error(&error)),
            }
        };
        if !self.transport.is_closed()
            && let Err(error) = self.transport.write(&response)
            && !self.transport.is_closed()
        {
            eprintln!("proteus-module-sdk: failed to write invocation response: {error:#}");
            std::process::exit(1);
        }
        if let Err(error) = self.finish_invocation(&invocation_id) {
            eprintln!("proteus-module-sdk: failed to settle invocation: {error:#}");
            std::process::exit(1);
        }
    }

    fn cancel(&self, method: String, params: Value) -> Result<()> {
        if method != PROCESS_MODULE_CANCEL_METHOD {
            bail!("unsupported component notification {method:?}");
        }
        let cancel: ProcessModuleCancel =
            serde_json::from_value(params).context("invalid cancellation params")?;
        self.validate_invocation_id(&cancel.invocation_id)?;
        let active = self
            .active
            .lock()
            .map_err(|_| anyhow!("active invocation map poisoned"))?;
        if let Some(invocation) = active.get(&cancel.invocation_id) {
            invocation.cancellation.store(true, Ordering::SeqCst);
        }
        // A terminal response and an already-sent cancellation may cross on
        // the duplex transport. Unknown completed ids are therefore harmless.
        Ok(())
    }

    fn complete_callback(
        &self,
        id: String,
        result: Result<Value, ProcessModuleRpcError>,
    ) -> Result<()> {
        let wire_id = parse_wire_id(&id)?;
        if wire_id.direction != WireDirection::Module
            || wire_id.generation != self.generation
            || wire_id.sequence == 0
        {
            bail!("callback response id has wrong direction or generation: {id:?}");
        }
        self.transport.complete_callback(&id, result)
    }

    fn validate_invocation_id(&self, id: &str) -> Result<()> {
        let wire_id = parse_wire_id(id)?;
        if wire_id.direction != WireDirection::Host
            || wire_id.generation != self.generation
            || wire_id.sequence == 0
        {
            bail!("invocation id has wrong direction or generation: {id:?}");
        }
        Ok(())
    }

    fn finish_invocation(&self, id: &str) -> Result<()> {
        self.active
            .lock()
            .map_err(|_| anyhow!("active invocation map poisoned"))?
            .remove(id)
            .ok_or_else(|| anyhow!("invocation {id:?} settled more than once"))?;
        Ok(())
    }
}

fn terminal_error(error: &anyhow::Error) -> ProcessModuleRpcError {
    let failure = error
        .downcast_ref::<proteus_contracts::process_module::ProcessModuleError>()
        .and_then(|error| error.model_failure.as_ref())
        .or_else(|| error.downcast_ref::<proteus_contracts::model_standard::ModelFailure>());
    let rpc = ProcessModuleRpcError::new(-32_000, format!("{error:#}"));
    match failure {
        Some(failure) => match serde_json::to_value(failure) {
            Ok(data) => rpc.with_data(data),
            Err(error) => ProcessModuleRpcError::new(
                -32603,
                format!("failed to serialize model failure: {error}"),
            ),
        },
        None => rpc,
    }
}

impl ModuleExports {
    fn dispatch(
        &self,
        call: ProcessComponentInvocation,
        method: &str,
        bridge: &HostBridge,
    ) -> Result<Value> {
        let export = self
            .exports
            .get(&(call.export.slot.clone(), call.export.module_id.clone()))
            .ok_or_else(|| {
                anyhow!(
                    "component {:?} has no export {}/{}",
                    self.component_id,
                    call.export.slot,
                    call.export.module_id
                )
            })?;
        let manifest = export.manifest();
        let authority = process_contract_authority(&manifest.slot, &manifest.contract_version)
            .expect("validated export authority");
        if !authority.allows_module_method(method) {
            bail!(
                "method {method:?} is not allowed for slot {}",
                manifest.slot
            );
        }
        export.invoke(method, call.params, bridge)
    }
}

fn validate_lineage(
    id: &str,
    lineage: &ProcessInvocationLineage,
    active: &HashMap<String, ActiveInvocation>,
    generation: u64,
) -> Result<()> {
    validate_lineage_id(&lineage.root_invocation_id, generation)?;
    if lineage.depth == 0 {
        if lineage.root_invocation_id != id || lineage.parent_invocation_id.is_some() {
            bail!("root invocation has inconsistent lineage");
        }
        return Ok(());
    }
    let parent_id = lineage
        .parent_invocation_id
        .as_deref()
        .ok_or_else(|| anyhow!("nested invocation is missing parent_invocation_id"))?;
    validate_lineage_id(parent_id, generation)?;
    let parent = active
        .get(parent_id)
        .ok_or_else(|| anyhow!("nested invocation names inactive parent {parent_id:?}"))?;
    if lineage.root_invocation_id != parent.lineage.root_invocation_id
        || lineage.depth != parent.lineage.depth.saturating_add(1)
    {
        bail!("nested invocation has inconsistent lineage");
    }
    Ok(())
}

fn validate_lineage_id(id: &str, generation: u64) -> Result<()> {
    let wire_id = parse_wire_id(id)?;
    if wire_id.direction != WireDirection::Host
        || wire_id.generation != generation
        || wire_id.sequence == 0
    {
        bail!("lineage contains wrong-generation invocation id {id:?}");
    }
    Ok(())
}

fn validate_initialize(binding: &ProcessComponentInitialize) -> Result<()> {
    if binding.protocol_version != PROCESS_COMPONENT_PROTOCOL_VERSION {
        bail!("unsupported process component protocol version");
    }
    if binding.component_id.trim().is_empty() {
        bail!("process component id must not be empty");
    }
    if binding.exports.is_empty() {
        bail!("process component must declare exports");
    }
    let mut seen = BTreeSet::new();
    for export in &binding.exports {
        if !export.module_config.is_object() {
            bail!(
                "process component export {}/{} config must be an object",
                export.slot,
                export.module_id
            );
        }
        if !seen.insert((export.slot.as_str(), export.module_id.as_str())) {
            bail!(
                "duplicate process component export {}/{}",
                export.slot,
                export.module_id
            );
        }
        let authority = process_contract_authority(&export.slot, &export.contract_version)
            .ok_or_else(|| {
                anyhow!(
                    "unknown process contract {}/{}",
                    export.slot,
                    export.contract_version
                )
            })?;
        if export.composition != authority.composition {
            bail!(
                "composition mismatch for {}/{}",
                export.slot,
                export.module_id
            );
        }
        let mut features = BTreeSet::new();
        for feature in &export.host_features {
            if feature.trim().is_empty() || !features.insert(feature.as_str()) {
                bail!(
                    "empty or duplicate host feature for {}/{}",
                    export.slot,
                    export.module_id
                );
            }
        }
        if authority
            .required_features
            .iter()
            .any(|feature| !features.contains(feature))
        {
            bail!(
                "host omitted a required feature for {}/{}",
                export.slot,
                export.module_id
            );
        }
    }
    Ok(())
}

fn validate_export_manifest(
    binding: &ProcessComponentExportInitialize,
    manifest: &proteus_contracts::contracts::ProcessComponentExportManifest,
) -> Result<()> {
    if manifest.slot != binding.slot
        || manifest.module_id != binding.module_id
        || manifest.contract_version != binding.contract_version
        || manifest.composition != binding.composition
    {
        bail!("export factory changed requested identity or contract");
    }
    let mut seen = BTreeSet::new();
    for feature in &manifest.module_features {
        if !binding.host_features.contains(feature) || !seen.insert(feature) {
            bail!("export confirmed an unoffered or duplicate feature {feature:?}");
        }
    }
    Ok(())
}

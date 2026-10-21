use anyhow::{Result, bail};
use proteus_module_sdk::{
    contracts::{
        contracts::{
            ProcessComponentExportInitialize, ProcessToolDefinition, ProcessToolInvokeInput,
        },
        domain::{ToolResult, ToolSafety, ToolSpec},
        process_module::ToolModuleHost,
    },
    tool::{ToolExport, ToolModule},
};
use serde::Deserialize;
use serde_json::json;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    prefix: String,
}

struct Echo(Config);

impl ToolModule for Echo {
    fn definitions(&self) -> Vec<ProcessToolDefinition> {
        vec![ProcessToolDefinition {
            spec: ToolSpec::new(
                "echo",
                "Return supplied text with the configured prefix",
                json!({
                    "type": "object", "properties": {"text": {"type": "string"}},
                    "required": ["text"], "additionalProperties": false
                }),
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
        if host.is_cancelled()? {
            bail!("canceled");
        }
        // ToolExport already validated the advertised JSON schema.
        let text = input.call.args["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("text must be a string"))?;
        Ok(ToolResult::ok(
            input.call.id,
            format!("{}{text}", self.0.prefix),
        ))
    }
}

fn load(binding: ProcessComponentExportInitialize) -> Result<Box<dyn proteus_module_sdk::Export>> {
    if binding.slot != "tool" || binding.module_id != "example.echo" {
        bail!("unsupported export {}/{}", binding.slot, binding.module_id);
    }
    let config = serde_json::from_value(binding.module_config.clone())?;
    Ok(Box::new(ToolExport::new(binding, Echo(config))?))
}

fn main() {
    if let Err(error) = proteus_module_sdk::run(load) {
        eprintln!("proteus-rust-tool-example: {error:#}");
        std::process::exit(1);
    }
}

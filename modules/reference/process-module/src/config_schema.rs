use proteus_contracts::domain::ModuleConfigSchema;
use serde_json::Value;

pub(crate) fn describe(slot: &str, id: &str, config: &Value) -> Option<ModuleConfigSchema> {
    Some(match slot {
        "model" => model_pack::adapters::config_schema::config_schema(config),
        "context" => context_pack::config_schema(id),
        "workflow" => coding_workflow::config_schema(id),
        "hook" if id == "tool_selection.dynamic" => tool_selection_hook::config_schema(),
        "policy" => policy_pack::config_schema(id),
        "hook" if id == "codex_compact" => codex_context_preparation::config_schema(),
        "hook" => hook_pack::config_schema(id),
        "tool" | "context_provider" if id == "jsonl_memory" => memory_pack::config_schema(),
        "tool" | "context_provider" if id == "sqlite_memory" => sqlite_memory::config_schema(),
        "tool" | "context_provider" if id == "rg_search" => ModuleConfigSchema::default(),
        "tool" if id == "codex_patch" => codex_patch::config_schema(),
        // These packs read no module config.
        "tool"
            if matches!(
                id,
                "file_tools"
                    | "git_tools"
                    | "plan_tool"
                    | "policy_tools"
                    | "rust_lsp"
                    | "shell_tools"
                    | "skill_tool"
                    | "direct_patch"
                    | "reference.tools"
            ) =>
        {
            ModuleConfigSchema::default()
        }
        "context_provider" if id == "skills" => ModuleConfigSchema::default(),
        _ => return None,
    })
}

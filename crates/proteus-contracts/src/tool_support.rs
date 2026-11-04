//! Общие helper-ы для process tool modules.
//!
//! Они живут в `proteus-contracts`, чтобы module crates не копировали JSON
//! serialization и workspace containment logic.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::process_module::{ProcessModuleError, ProcessModuleResult, ToolModuleInvocationContext};

mod workspace_write;
pub use workspace_write::{WorkspaceFile, workspace_file_for_write};

/// Входные данные tool: разбор serialized `ToolCall` JSON.
#[derive(Debug, Deserialize)]
pub struct ToolCallDto {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub args: Value,
}

/// Парсит `ToolCall` из JSON-строки.
pub fn parse_call(call_json: &str) -> Result<ToolCallDto, String> {
    serde_json::from_str(call_json).map_err(|e| format!("failed to parse ToolCall: {e}"))
}

/// Парсит обязательный runtime context одного process tool invoke.
pub fn parse_invocation_context(context_json: &str) -> Result<ToolModuleInvocationContext, String> {
    serde_json::from_str(context_json)
        .map_err(|e| format!("failed to parse ToolModuleInvocationContext: {e}"))
}

/// Сериализует успешный результат в JSON `ToolResult`.
pub fn ok_result(
    call_id: &str,
    tool_name: &str,
    output: String,
    metadata: Value,
) -> ProcessModuleResult<String> {
    let result = json!({
        "call_id": call_id,
        "ok": true,
        "output": output,
        "content": [],
        "error": null,
        "metadata": add_tool_name(metadata, tool_name),
    });
    Ok(result.to_string())
}

/// Сериализует ошибку как `ToolResult` с `ok=false`.
pub fn err_result(call_id: &str, tool_name: &str, error: String) -> ProcessModuleResult<String> {
    let result = json!({
        "call_id": call_id,
        "ok": false,
        "output": "",
        "content": [],
        "error": error,
        "metadata": { "tool": tool_name },
    });
    Ok(result.to_string())
}

/// Module-level error (не тот, что `ToolResult.error`). Используется когда
/// tool не смог вообще выполниться - невалидный JSON call, crash и т.п.
pub fn module_error(message: String) -> ProcessModuleResult<String> {
    Err(ProcessModuleError::new(message))
}

fn add_tool_name(mut metadata: Value, tool_name: &str) -> Value {
    if let Some(obj) = metadata.as_object_mut() {
        obj.insert("tool".to_owned(), json!(tool_name));
        Value::Object(obj.clone())
    } else {
        json!({ "tool": tool_name })
    }
}

pub fn required_string<'a>(args: &'a Value, key: &str, tool_name: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("tool '{tool_name}' requires string arg '{key}'"))
}

pub fn optional_positive_usize(
    args: &Value,
    key: &str,
    tool_name: &str,
) -> Result<Option<usize>, String> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    let Some(number) = value.as_u64() else {
        return Err(format!("tool '{tool_name}' requires integer arg '{key}'"));
    };
    if number == 0 {
        return Err(format!(
            "tool '{tool_name}' requires positive integer arg '{key}'"
        ));
    }
    Ok(Some(number as usize))
}

pub fn optional_string_array(
    args: &Value,
    key: &str,
    tool_name: &str,
) -> Result<Vec<String>, String> {
    let Some(value) = args.get(key) else {
        return Ok(Vec::new());
    };
    let Some(values) = value.as_array() else {
        return Err(format!("tool '{tool_name}' requires array arg '{key}'"));
    };
    let mut strings = Vec::with_capacity(values.len());
    for value in values {
        let Some(item) = value.as_str() else {
            return Err(format!(
                "tool '{tool_name}' requires array arg '{key}' to contain only strings"
            ));
        };
        strings.push(item.to_owned());
    }
    Ok(strings)
}

/// Проверяет, что `path` лежит внутри `cwd`. Возвращает canonical path.
pub fn workspace_path(cwd: &Path, path: &Path) -> Result<PathBuf, String> {
    let base = std::fs::canonicalize(cwd)
        .map_err(|e| format!("failed to canonicalize cwd {}: {e}", cwd.display()))?;
    let target = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let canonical = std::fs::canonicalize(&target)
        .map_err(|e| format!("failed to canonicalize {}: {e}", target.display()))?;
    if !canonical.starts_with(&base) {
        return Err(format!("path escapes workspace: {}", path.display()));
    }
    Ok(canonical)
}

fn safe_relative_path(path: &Path) -> Result<PathBuf, String> {
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("path escapes workspace: {}", path.display()));
            }
        }
    }
    if safe.as_os_str().is_empty() {
        return Err(format!("no file name in {}", path.display()));
    }
    Ok(safe)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{contracts::ExecutionAttribution, domain::new_execution_id};

    use super::*;

    #[test]
    fn invocation_context_is_required_and_strict() {
        let context = ToolModuleInvocationContext {
            skills: Default::default(),
            cwd: PathBuf::from("/workspace"),
            attribution: ExecutionAttribution::detached(new_execution_id()),
            config: json!({}),
        };
        let value = serde_json::to_value(&context).expect("context value");
        parse_invocation_context(&value.to_string()).expect("valid context");

        let mut missing_attribution = value.clone();
        missing_attribution
            .as_object_mut()
            .expect("context object")
            .remove("attribution");
        assert!(parse_invocation_context(&missing_attribution.to_string()).is_err());

        let mut unknown_field = value;
        unknown_field
            .as_object_mut()
            .expect("context object")
            .insert("legacy_cwd".to_owned(), json!("/workspace"));
        assert!(parse_invocation_context(&unknown_field.to_string()).is_err());
    }

    #[test]
    fn write_path_rejects_parent_escape() {
        let dir = tempfile::tempdir().expect("workspace");
        let error = workspace_file_for_write(dir.path(), Path::new("../outside.txt"))
            .expect_err("parent traversal must be rejected");
        assert!(error.contains("escapes workspace"), "{error}");
    }

    #[test]
    fn write_path_creates_missing_parent_dirs() {
        let dir = tempfile::tempdir().expect("workspace");
        let path =
            workspace_file_for_write(dir.path(), Path::new("a/b/out.txt")).expect("write file");

        assert_eq!(path.path(), dir.path().join("a/b/out.txt"));
        assert!(dir.path().join("a/b").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn write_path_rejects_existing_symlink() {
        let dir = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let outside_file = outside.path().join("secret.txt");
        std::fs::write(&outside_file, "secret").expect("outside file");
        std::os::unix::fs::symlink(&outside_file, dir.path().join("link.txt")).expect("symlink");

        let error = workspace_file_for_write(dir.path(), Path::new("link.txt"))
            .expect_err("symlink target must be rejected");
        assert!(error.contains("symlink"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn write_path_rejects_symlink_parent() {
        let dir = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link_dir")).expect("symlink");

        let error = workspace_file_for_write(dir.path(), Path::new("link_dir/out.txt"))
            .expect_err("symlink parent must be rejected");
        assert!(error.contains("symlink"), "{error}");
    }
}

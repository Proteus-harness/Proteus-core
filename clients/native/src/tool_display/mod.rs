//! How tool calls read in the thread: an action label, the subject it acts on
//! and the remaining arguments. Tool names are matched only here, so a
//! replaced toolset degrades to the generic headline instead of breaking.

mod patch;
#[cfg(test)]
mod tests;

use serde_json::Value;

#[cfg(test)]
use patch::PatchOperation;
pub use patch::{DiffLine, PatchFile, patch_files, patch_stats};

pub const APPLY_PATCH: &str = "apply_patch";
pub const UPDATE_PLAN: &str = "update_plan";

/// Conventional argument names that usually carry the subject of an unknown tool.
const SUBJECT_KEYS: [&str; 8] = [
    "cmd",
    "command",
    "path",
    "file_path",
    "paths",
    "pattern",
    "query",
    "url",
];
const SUBJECT_LIMIT: usize = 300;
const ARG_LIMIT: usize = 160;
const MAX_ARGS: usize = 6;

/// Action label and the argument shown as its subject; `""` means no subject.
fn known_tool(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "exec_command" => ("Команда", "cmd"),
        "shell" => ("Команда", "command"),
        "write_stdin" => ("Ввод в команду", "chars"),
        "read_file" => ("Чтение", "path"),
        "read_many_files" => ("Чтение", "paths"),
        "write_file" => ("Запись", "path"),
        "edit_file" => ("Правка", "path"),
        "list_dir" => ("Список файлов", "path"),
        "find_files" => ("Поиск файлов", "pattern"),
        "grep" => ("Поиск", "pattern"),
        "git_status" => ("Статус git", ""),
        "git_diff" => ("Изменения git", "path"),
        "git_log" => ("История git", ""),
        "web_search" => ("Поиск в сети", "query"),
        "request_permissions" => ("Запрос прав", "justification"),
        "view_image" => ("Изображение", "path"),
        APPLY_PATCH => ("Правка", ""),
        UPDATE_PLAN => ("План", ""),
        _ => return None,
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Headline {
    pub label: String,
    pub subject: Option<String>,
    /// Lines added and removed by a patch.
    pub stats: Option<(usize, usize)>,
    /// Argument already shown as the subject; details list only the rest.
    pub subject_key: Option<String>,
}

pub fn headline(name: &str, args: &Value) -> Headline {
    let known = known_tool(name);
    let label = known.map_or_else(|| name.to_owned(), |(label, _)| label.to_owned());
    if name == APPLY_PATCH
        && let Some(patch) = patch_text(args)
    {
        let files = patch_files(patch);
        if !files.is_empty() {
            return Headline {
                label,
                subject: Some(patch_subject(&files)),
                stats: Some(patch_stats(&files)),
                subject_key: None,
            };
        }
    }
    if name == UPDATE_PLAN {
        let steps = plan_steps(args);
        if !steps.is_empty() {
            return Headline {
                label,
                subject: Some(plan_summary(&steps)),
                stats: None,
                subject_key: None,
            };
        }
    }
    let key = match known {
        Some((_, "")) => None,
        Some((_, key)) => Some(key.to_owned()),
        None => generic_subject_key(args),
    };
    let subject = key
        .as_deref()
        .and_then(|key| args.get(key).and_then(|value| subject_text(key, value)));
    Headline {
        label,
        subject_key: subject.as_ref().and(key),
        subject,
        stats: None,
    }
}

/// Arguments not already shown in the headline, as short `key: value` pairs.
pub fn other_args(args: &Value, subject_key: Option<&str>) -> Vec<(String, String)> {
    let Some(map) = args.as_object() else {
        return Vec::new();
    };
    map.iter()
        .filter(|(key, value)| !value.is_null() && Some(key.as_str()) != subject_key)
        .take(MAX_ARGS)
        .map(|(key, value)| (key.clone(), arg_preview(value)))
        .collect()
}

pub fn patch_text(args: &Value) -> Option<&str> {
    args.get("patch")
        .or_else(|| args.get("input"))
        .and_then(Value::as_str)
        .filter(|patch| !patch.trim().is_empty())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanStep {
    pub step: String,
    pub status: StepStatus,
}

pub fn plan_steps(args: &Value) -> Vec<PlanStep> {
    let Some(steps) = args.get("plan").and_then(Value::as_array) else {
        return Vec::new();
    };
    steps
        .iter()
        .filter_map(|entry| {
            let step = entry.get("step").and_then(Value::as_str)?;
            let status = match entry.get("status").and_then(Value::as_str)? {
                "completed" => StepStatus::Completed,
                "in_progress" => StepStatus::InProgress,
                _ => StepStatus::Pending,
            };
            Some(PlanStep {
                step: step.to_owned(),
                status,
            })
        })
        .collect()
}

fn plan_summary(steps: &[PlanStep]) -> String {
    let completed = steps
        .iter()
        .filter(|step| step.status == StepStatus::Completed)
        .count();
    let current = steps
        .iter()
        .find(|step| step.status == StepStatus::InProgress)
        .map(|step| format!(" · {}", step.step))
        .unwrap_or_default();
    limit(
        format!("{completed}/{}{current}", steps.len()),
        SUBJECT_LIMIT,
    )
}

fn patch_subject(files: &[PatchFile]) -> String {
    match files {
        [only] => only.path.clone(),
        [first, rest @ ..] => format!("{} +{}", first.path, rest.len()),
        [] => String::new(),
    }
}

fn generic_subject_key(args: &Value) -> Option<String> {
    let map = args.as_object()?;
    SUBJECT_KEYS
        .iter()
        .find(|key| {
            map.get(**key)
                .is_some_and(|value| subject_text(key, value).is_some())
        })
        .map(|key| (*key).to_owned())
        .or_else(|| {
            map.iter()
                .find(|(_, value)| {
                    value.as_str().is_some_and(|text| {
                        !text.trim().is_empty()
                            && !text.contains('\n')
                            && text.chars().count() <= 120
                    })
                })
                .map(|(key, _)| key.clone())
        })
}

fn subject_text(key: &str, value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => single_line(text)?,
        Value::Array(items) => {
            let parts = items.iter().filter_map(Value::as_str).collect::<Vec<_>>();
            match parts.as_slice() {
                [] => return None,
                [only] => single_line(only)?,
                [first, rest @ ..] if key.ends_with('s') => format!("{first} +{}", rest.len()),
                _ => parts.join(" "),
            }
        }
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    Some(limit(text, SUBJECT_LIMIT))
}

fn single_line(text: &str) -> Option<String> {
    let mut lines = text.trim().lines();
    let first = lines.next()?.trim_end();
    if first.is_empty() {
        return None;
    }
    Some(if lines.next().is_some() {
        format!("{first} …")
    } else {
        first.to_owned()
    })
}

fn arg_preview(value: &Value) -> String {
    match value {
        Value::String(text) => match single_line(text) {
            Some(line) => limit(line, ARG_LIMIT),
            None => String::new(),
        },
        Value::Array(items) => format!("[{}]", items.len()),
        Value::Object(map) => format!("{{{}}}", map.len()),
        other => other.to_string(),
    }
}

fn limit(text: String, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    }
}

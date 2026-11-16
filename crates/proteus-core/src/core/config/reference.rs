//! Physical config source and inline child selection share one stable reference.
//! Only the reserved `#agent=` suffix has special meaning; other `#` paths stay
//! ordinary file names. The selector uses JSON pointer escaping for role names.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

const MARKER: &str = "#agent=";

pub fn physical_config_path(path: &Path) -> PathBuf {
    path.to_str()
        .and_then(|path| path.split_once(MARKER))
        .map_or_else(|| path.to_path_buf(), |(file, _)| PathBuf::from(file))
}

pub fn config_selection(path: &Path) -> Result<Vec<String>> {
    let Some((file, pointer)) = path.to_str().and_then(|path| path.split_once(MARKER)) else {
        return Ok(Vec::new());
    };
    if file.is_empty() || !pointer.starts_with('/') {
        bail!("inline agent config reference must be FILE#agent=/ROLE");
    }
    pointer[1..]
        .split('/')
        .map(|token| {
            let mut value = String::new();
            let mut chars = token.chars();
            while let Some(ch) = chars.next() {
                if ch != '~' {
                    value.push(ch);
                } else {
                    value.push(match chars.next() {
                        Some('0') => '~',
                        Some('1') => '/',
                        _ => bail!("invalid JSON pointer escape in inline agent config reference"),
                    });
                }
            }
            if value.trim().is_empty() {
                bail!("inline agent config reference must name every selected role");
            }
            Ok(value)
        })
        .collect()
}

pub fn config_reference(parent: &Path, role: &str) -> Result<PathBuf> {
    if role.trim().is_empty() {
        bail!("inline agent role name must not be empty");
    }
    let mut selection = config_selection(parent)?;
    selection.push(role.to_owned());
    let file = physical_config_path(parent);
    let file = file
        .to_str()
        .context("inline agent config source must have a UTF-8 path")?;
    if file.is_empty() {
        bail!("inline agent config reference must name its source file");
    }
    let pointer = selection
        .iter()
        .map(|name| name.replace('~', "~0").replace('/', "~1"))
        .collect::<Vec<_>>()
        .join("/");
    Ok(PathBuf::from(format!("{file}{MARKER}/{pointer}")))
}

pub fn selected_config_value_mut<'a>(
    mut source: &'a mut Value,
    selection: &[String],
) -> Result<&'a mut Value> {
    for name in selection {
        let roles = source
            .get_mut("agent_control")
            .and_then(|control| control.get_mut("roles"))
            .and_then(Value::as_array_mut)
            .with_context(|| format!("inline agent role {name:?} was not found"))?;
        let mut found = None;
        for (index, role) in roles.iter().enumerate() {
            if role.get("name").and_then(Value::as_str) == Some(name) {
                if found.replace(index).is_some() {
                    bail!("duplicate inline agent role {name:?}");
                }
            }
        }
        source = roles
            [found.with_context(|| format!("inline agent role {name:?} was not found"))?]
        .get_mut("config")
        .filter(|config| config.is_object())
        .with_context(|| format!("agent role {name:?} must contain an inline config object"))?;
    }
    Ok(source)
}

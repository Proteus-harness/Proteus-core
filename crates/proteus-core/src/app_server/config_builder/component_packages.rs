//! Host-owned registration of built packages, inside the profile transaction.
use super::*;
use proteus_contracts::app_protocol::http::ComponentPackageInstall;

pub(super) fn apply(
    config: &mut AppConfig,
    packages: Vec<ComponentPackageInstall>,
) -> Result<BTreeSet<String>> {
    let mut touched = BTreeSet::new();
    for package in packages {
        validate_identity(&package.id, "component package id")?;
        let (command, args) = package
            .command
            .split_first()
            .ok_or_else(|| anyhow!("component package command must not be empty"))?;
        let executable = Path::new(command);
        if !executable.is_absolute()
            || executable.file_name().is_none()
            || package.command.iter().any(|arg| arg.contains('\0'))
        {
            anyhow::bail!("component package command must name an absolute executable without NUL");
        }
        if package.exports.is_empty() {
            anyhow::bail!("component package exports must not be empty");
        }
        let mut exports = BTreeMap::new();
        for (slot, ids) in &package.exports {
            validate_identity(slot, "component package slot")?;
            if ids.is_empty() {
                anyhow::bail!("component package exports.{slot} must not be empty");
            }
            let mut entries = BTreeMap::new();
            for id in ids {
                validate_identity(id, "component package export id")?;
                if entries.insert(id.clone(), serde_json::json!({})).is_some() {
                    anyhow::bail!("duplicate component package export {slot}/{id}");
                }
            }
            exports.insert(slot.clone(), entries);
        }
        if package.replace.is_empty() {
            if !touched.insert(package.id.clone()) || config.components.contains_key(&package.id) {
                anyhow::bail!("component already exists: {}", package.id);
            }
            let component: crate::process_adapters::ProcessComponentConfig =
                serde_json::from_value(serde_json::json!({
                    "command": command, "args": args, "exports": exports,
                }))?;
            component.validate_for(&package.id, &format!("components.{}", package.id))?;
            config.components.insert(package.id, component);
        } else {
            for replacement in package.replace {
                if !touched.insert(replacement.id.clone()) {
                    anyhow::bail!("component replaced more than once: {}", replacement.id);
                }
                let component = config.components.get_mut(&replacement.id).ok_or_else(|| {
                    anyhow!("component to replace does not exist: {}", replacement.id)
                })?;
                if component.command() != replacement.command {
                    anyhow::bail!(
                        "component command changed since package selection: {}",
                        replacement.id
                    );
                }
                if Path::new(component.command()).file_name() != executable.file_name() {
                    anyhow::bail!(
                        "component executable does not match package: {}",
                        replacement.id
                    );
                }
                for (slot, id, _) in component.exports() {
                    if !exports
                        .get(slot)
                        .is_some_and(|entries| entries.contains_key(id))
                    {
                        anyhow::bail!("package does not declare component export {slot}/{id}");
                    }
                }
                // Explicit aliases retain args, sandbox, cwd/env, descriptions and
                // their selected export subset, including per-export controls.
                component.set_command(command.clone());
            }
        }
    }
    Ok(touched)
}

fn validate_identity(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        anyhow::bail!(
            "{field} must be a nonempty identity without surrounding whitespace or controls"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;

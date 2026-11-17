//! Source-owned model connection removal within the profile transaction.
use super::*;

pub(super) fn remove(
    config: &mut AppConfig,
    source: &Path,
    ids: &[String],
) -> Result<BTreeSet<String>> {
    let mut touched = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for id in ids {
        anyhow::ensure!(
            !id.is_empty() && id.trim() == id && !id.chars().any(char::is_control),
            "model connection id must be nonempty without whitespace or controls"
        );
        anyhow::ensure!(seen.insert(id), "duplicate model connection removal {id:?}");
        let users: Vec<_> = config
            .providers
            .iter()
            .filter(|(_, preset)| preset.provider == *id)
            .map(|(name, _)| name.as_str())
            .collect();
        anyhow::ensure!(
            users.is_empty(),
            "model connection {id:?} is used by model presets {}",
            users.join(", ")
        );
        let owners: Vec<_> = config
            .components
            .iter()
            .filter(|(_, component)| {
                component
                    .exports()
                    .any(|(slot, export, _)| slot == "model" && export == id)
            })
            .map(|(name, _)| name.clone())
            .collect();
        anyhow::ensure!(
            owners.len() == 1,
            "model connection {id:?} is unavailable or ambiguous"
        );
        let target =
            config_builder_target_path(Some(source)).context("config source is unavailable")?;
        AppConfig::ensure_local_model_connection(&target, id)?;
        let owner = &owners[0];
        let component = config.components.get_mut(owner).expect("owner checked");
        component.remove_export("model", id);
        if component.exports().next().is_none() {
            config.components.remove(owner);
        }
        if let Some(settings) = config.module_config.get_mut("model") {
            settings.remove(id);
        }
        touched.insert(owner.clone());
    }
    Ok(touched)
}

#[cfg(test)]
mod tests;

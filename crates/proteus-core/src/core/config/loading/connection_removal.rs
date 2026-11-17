//! Validates a local connection removal using the existing include merge.
use super::*;

impl AppConfig {
    /// Check removal against the authored document and its include merge.
    /// An export supplied again by another source must be edited in that source.
    pub(crate) fn ensure_local_model_connection(path: &Path, id: &str) -> Result<()> {
        let selection = config_selection(path)?;
        let physical = physical_config_path(path);
        let mut local = load_config_value(&physical)?;
        let selected = selected_config_value_mut(&mut local, &selection)?;
        let components = selected
            .get_mut("components")
            .and_then(Value::as_object_mut)
            .context("model connection is inherited; edit its source configuration")?;
        let mut found = false;
        for component in components.values_mut() {
            if let Some(exports) = component
                .get_mut("exports")
                .and_then(|exports| exports.get_mut("model"))
                .and_then(Value::as_object_mut)
            {
                found |= exports.remove(id).is_some();
            }
        }
        anyhow::ensure!(
            found,
            "model connection {id:?} is inherited; edit its source configuration"
        );
        let mut stack = BTreeSet::new();
        let mut inputs = ConfigInputs::default();
        let mut effective = if physical.file_name().and_then(|name| name.to_str())
            == Some(super::super::CONFIG_BUILDER_OVERLAY)
        {
            let parent = physical.parent().context("config overlay has no parent")?;
            let mut merged = Value::Object(Map::new());
            for file in super::super::config_files(Some(parent))
                .into_iter()
                .filter(|file| *file != physical)
            {
                merge_config_value(
                    &mut merged,
                    load_config_path_value(&file, &mut stack, &mut inputs)?,
                );
            }
            merge_config_value(
                &mut merged,
                merge_config_file_value(&physical, local, &mut stack, &mut inputs)?,
            );
            merged
        } else {
            merge_config_file_value(&physical, local, &mut stack, &mut inputs)?
        };
        let selected = selected_config_value_mut(&mut effective, &selection)?;
        let inherited = selected
            .get("components")
            .and_then(Value::as_object)
            .is_some_and(|components| {
                components.values().any(|component| {
                    component
                        .get("exports")
                        .and_then(|exports| exports.get("model"))
                        .and_then(Value::as_object)
                        .is_some_and(|exports| exports.contains_key(id))
                })
            });
        anyhow::ensure!(
            !inherited,
            "model connection {id:?} is also inherited; edit its source configuration"
        );
        Ok(())
    }
}

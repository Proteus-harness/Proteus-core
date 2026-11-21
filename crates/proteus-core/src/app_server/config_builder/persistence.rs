use super::*;

async fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let temporary = path.with_file_name(format!(
        ".config-builder-{}.tmp",
        crate::domain::new_call_id()
    ));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await?;
        if let Ok(metadata) = tokio::fs::metadata(path).await {
            file.set_permissions(metadata.permissions()).await?;
        }
        file.write_all(bytes).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temporary, path).await?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}

pub(in crate::app_server) fn config_builder_target_path(
    config_path: Option<&Path>,
) -> Option<PathBuf> {
    let path = config_path?;
    let selection = crate::core::config_selection(path).ok()?;
    let physical = crate::core::physical_config_path(path);
    let mut target = if physical.is_dir() {
        if !selection.is_empty() {
            return None;
        }
        physical.join(crate::core::CONFIG_BUILDER_OVERLAY)
    } else {
        std::fs::canonicalize(&physical).unwrap_or(physical)
    };
    for role in selection {
        target = crate::core::config_reference(&target, &role).ok()?;
    }
    Some(target)
}

#[derive(serde::Serialize)]
struct ModuleConfigToml<'a> {
    module_config: &'a BTreeMap<String, BTreeMap<String, Value>>,
}

pub(super) fn validate_module_config_toml(
    module_config: &BTreeMap<String, BTreeMap<String, Value>>,
) -> Result<()> {
    module_config_toml_document(module_config).map(|_| ())
}

fn module_config_toml_document(
    module_config: &BTreeMap<String, BTreeMap<String, Value>>,
) -> Result<toml_edit::DocumentMut> {
    let text = toml::to_string_pretty(&ModuleConfigToml { module_config })
        .context("module_config contains values that cannot be represented as TOML")?;
    text.parse::<toml_edit::DocumentMut>()
        .context("serialized module_config TOML could not be parsed")
}

/// Initial source must retain file references, not their resolved runtime text.
fn initial_profile_source(config: &AppConfig) -> AppConfig {
    let mut source = config.clone();
    for entry in &mut source.instructions {
        if entry.file.is_some() {
            entry.text = None;
        }
    }
    source
}

#[cfg(test)]
pub(super) async fn persist_config_builder(path: &Path, config: &AppConfig) -> Result<()> {
    persist_config_builder_with_components(path, config, &BTreeSet::new()).await
}

pub(super) async fn persist_config_builder_with_components(
    path: &Path,
    config: &AppConfig,
    component_ids: &BTreeSet<String>,
) -> Result<()> {
    let selection = crate::core::config_selection(path)?;
    let physical = crate::core::physical_config_path(path);
    if let Some(parent) = physical.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    if physical
        .extension()
        .is_some_and(|extension| extension == "json")
    {
        return persist_json_profile(path, config, component_ids).await;
    }

    let mut doc = if !tokio::fs::try_exists(&physical).await?
        && physical.file_name().and_then(|name| name.to_str())
            != Some(crate::core::CONFIG_BUILDER_OVERLAY)
    {
        anyhow::ensure!(selection.is_empty(), "inline-agent source is unavailable");
        super::inline_source::ensure_bootstrap_has_no_inline_references(config)?;
        toml::to_string_pretty(&initial_profile_source(config))
            .context("cannot serialize initial profile")?
            .parse::<toml_edit::DocumentMut>()?
    } else {
        read_toml_document_or_empty(&physical).await?
    };
    let mut outer = None;
    if !selection.is_empty() {
        let selected = super::inline_source::selected_toml_document(&mut doc, &selection)?;
        outer = Some(doc);
        doc = selected;
    }

    let baseline = source_baseline(path).await?;
    if model_source_changed(config, baseline.as_ref())?.0
        && doc.get("model").is_none()
        && doc.get("active_provider").is_none()
        && let Some(outer) = &mut outer
    {
        super::inline_source::remove_selected_role_model(outer, &selection)?;
    }
    persist_toml_model_source(&mut doc, config, baseline.as_ref(), !selection.is_empty())?;
    if doc
        .get("permissions")
        .is_none_or(|item| !item.is_table_like())
    {
        doc["permissions"] = toml_edit::table();
    }
    doc["permissions"]["mode"] = toml_edit::value(permission_mode_str(config.permissions.mode));

    doc["modules"] = toml_edit::table();
    for (kind, id) in config.modules.iter() {
        doc["modules"][kind.as_str()] = toml_edit::value(id.to_owned());
    }

    doc["modules"]["hooks"] = toml_edit::value(
        config
            .modules
            .hooks
            .iter()
            .cloned()
            .collect::<toml_edit::Array>(),
    );

    if doc
        .get("agent_control")
        .is_none_or(|item| !item.is_table_like())
    {
        doc["agent_control"] = toml_edit::table();
    }
    doc["agent_control"]["surface"] = toml_edit::value(config.agent_control.surface.as_str());

    let omit_model_settings = !selection.is_empty()
        && omitted_model_settings_unchanged(
            config,
            baseline.as_ref(),
            doc.get("module_config")
                .and_then(|item| item.get("model"))
                .is_some(),
        );
    let mut module_config_doc = module_config_toml_document(&config.module_config)?;
    if omit_model_settings {
        if let Some(table) = module_config_doc
            .get_mut("module_config")
            .and_then(toml_edit::Item::as_table_like_mut)
        {
            table.remove("model");
        }
    }
    if let Some(item) = module_config_doc.as_table().get("module_config") {
        doc["module_config"] = item.clone();
    } else {
        doc["module_config"] = toml_edit::table();
    }

    if doc.get("tools").is_none_or(|item| !item.is_table_like()) {
        doc["tools"] = toml_edit::table();
    }
    doc["tools"]["enabled"] = toml_edit::value(
        config
            .tools
            .enabled
            .iter()
            .cloned()
            .collect::<toml_edit::Array>(),
    );

    #[derive(serde::Serialize)]
    struct AddonsToml<'a> {
        addons: &'a crate::domain::AddonConfig,
        tools: McpToolsToml<'a>,
    }
    #[derive(serde::Serialize)]
    struct McpToolsToml<'a> {
        mcp_servers: &'a [crate::domain::ConfiguredMcpServerConfig],
    }
    let managed = toml::to_string_pretty(&AddonsToml {
        addons: &config.addons,
        tools: McpToolsToml {
            mcp_servers: &config.tools.mcp_servers,
        },
    })
    .context("addon settings cannot be represented as TOML")?
    .parse::<toml_edit::DocumentMut>()?;
    doc["addons"] = managed["addons"].clone();
    doc["tools"]["mcp_servers"] = managed["tools"]["mcp_servers"].clone();
    if !component_ids.is_empty() {
        // Persist only installed aliases, keeping unrelated inherited descriptors
        // driven by their fragments. Full touched descriptors retain launch controls.
        #[derive(serde::Serialize)]
        struct ComponentsToml<'a> {
            components: BTreeMap<&'a String, &'a crate::process_adapters::ProcessComponentConfig>,
        }
        let components = toml::to_string_pretty(&ComponentsToml {
            components: component_ids
                .iter()
                .map(|id| (id, &config.components[id]))
                .collect(),
        })
        .context("components cannot be represented as TOML")?
        .parse::<toml_edit::DocumentMut>()?;
        if doc.get("components").is_none() {
            doc["components"] = toml_edit::table();
        }
        for id in component_ids {
            doc["components"][id] = components["components"][id].clone();
        }
    }
    if let Some(mut source) = outer {
        super::inline_source::replace_selected_toml_document(&mut source, &selection, doc)?;
        atomic_write(&physical, source.to_string().as_bytes()).await?;
    } else {
        atomic_write(&physical, doc.to_string().as_bytes()).await?;
    }
    Ok(())
}

pub(super) async fn read_toml_document_or_empty(path: &Path) -> Result<toml_edit::DocumentMut> {
    let existing = match tokio::fs::read_to_string(path).await {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read config {}", path.display()));
        }
    };
    existing
        .parse::<toml_edit::DocumentMut>()
        .map_err(|err| anyhow!("failed to parse config TOML at {}: {err}", path.display()))
}

/// Preserve unmanaged JSON keys just as the TOML overlay writer does.
async fn persist_json_profile(
    path: &Path,
    config: &AppConfig,
    component_ids: &BTreeSet<String>,
) -> Result<()> {
    let selection = crate::core::config_selection(path)?;
    let physical = crate::core::physical_config_path(path);
    let next = serde_json::to_value(config)?;
    let mut document: Value = match tokio::fs::read(&physical).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to parse config JSON at {}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            anyhow::ensure!(selection.is_empty(), "inline-agent source is unavailable");
            super::inline_source::ensure_bootstrap_has_no_inline_references(config)?;
            serde_json::to_value(initial_profile_source(config))?
        }
        Err(error) => return Err(error.into()),
    };
    let baseline = source_baseline(path).await?;
    if model_source_changed(config, baseline.as_ref())?.0 && !selection.is_empty() {
        let child = crate::core::selected_config_value_mut(&mut document, &selection)?;
        if child.get("model").is_none() && child.get("active_provider").is_none() {
            let (name, parents) = selection.split_last().unwrap();
            if let Some(roles) = crate::core::selected_config_value_mut(&mut document, parents)?
                .get_mut("agent_control")
                .and_then(|control| control.get_mut("roles"))
                .and_then(Value::as_array_mut)
            {
                if let Some(role) = roles
                    .iter_mut()
                    .find(|role| role.get("name").and_then(Value::as_str) == Some(name.as_str()))
                {
                    role.as_object_mut()
                        .context("agent role must be an object")?
                        .remove("model");
                }
            }
        }
    }
    let root = crate::core::selected_config_value_mut(&mut document, &selection)?
        .as_object_mut()
        .context("config JSON root must be an object")?;
    persist_json_model_source(root, config, baseline.as_ref(), !selection.is_empty())?;
    let omit_model_settings = !selection.is_empty()
        && omitted_model_settings_unchanged(
            config,
            baseline.as_ref(),
            root.get("module_config")
                .and_then(|item| item.get("model"))
                .is_some(),
        );
    for key in ["modules", "module_config", "addons"] {
        root.insert(key.into(), next[key].clone());
    }
    if omit_model_settings {
        if let Some(settings) = root.get_mut("module_config").and_then(Value::as_object_mut) {
            settings.remove("model");
        }
    }
    for (section, keys) in [
        ("permissions", &["mode"][..]),
        ("agent_control", &["surface"][..]),
        ("tools", &["enabled", "mcp_servers"][..]),
    ] {
        let section_value = root.entry(section).or_insert_with(|| serde_json::json!({}));
        let section_value = section_value
            .as_object_mut()
            .with_context(|| format!("config JSON {section} must be an object"))?;
        for key in keys {
            section_value.insert((*key).into(), next[section][*key].clone());
        }
    }
    if !component_ids.is_empty() {
        let components = root
            .entry("components")
            .or_insert_with(|| serde_json::json!({}));
        let components = components
            .as_object_mut()
            .context("config JSON components must be an object")?;
        for id in component_ids {
            components.insert(id.clone(), next["components"][id].clone());
        }
    }
    atomic_write(&physical, &serde_json::to_vec_pretty(&document)?).await
}

async fn source_baseline(path: &Path) -> Result<Option<AppConfig>> {
    let physical = crate::core::physical_config_path(path);
    if physical.file_name().and_then(|name| name.to_str())
        == Some(crate::core::CONFIG_BUILDER_OVERLAY)
    {
        return AppConfig::load(physical.parent()).await.map(Some);
    }
    if !tokio::fs::try_exists(&physical).await? {
        return Ok(None);
    }
    AppConfig::load(Some(path)).await.map(Some)
}

fn omitted_model_settings_unchanged(
    config: &AppConfig,
    baseline: Option<&AppConfig>,
    source_has_model_settings: bool,
) -> bool {
    !source_has_model_settings
        && baseline.is_some_and(|baseline| {
            config.module_config.get("model") == baseline.module_config.get("model")
        })
}

fn model_source_changed(config: &AppConfig, baseline: Option<&AppConfig>) -> Result<(bool, bool)> {
    let Some(baseline) = baseline else {
        return Ok((true, false));
    };
    Ok((
        config.active_provider != baseline.active_provider,
        serde_json::to_value(&config.providers)? != serde_json::to_value(&baseline.providers)?,
    ))
}

fn persist_toml_model_source(
    document: &mut toml_edit::DocumentMut,
    config: &AppConfig,
    baseline: Option<&AppConfig>,
    inline: bool,
) -> Result<()> {
    let (choice_changed, presets_changed) = model_source_changed(config, baseline)?;
    let named = document.get("models").is_some() || document.get("model").is_some();
    let had_providers = document.get("providers").is_some();
    // Unrelated edits must not turn the loader's inherited bootstrap choice
    // into a permanently pinned child model.
    let source_has_choice =
        document.get("model").is_some() || document.get("active_provider").is_some();
    if named || (!source_has_choice && !had_providers && (choice_changed || presets_changed)) {
        document.remove("active_provider");
        document.remove("providers");
        if presets_changed || had_providers {
            #[derive(serde::Serialize)]
            struct Models<'a> {
                models: &'a BTreeMap<String, crate::core::ProviderProfileConfig>,
            }
            let serialized = toml::to_string_pretty(&Models {
                models: &config.providers,
            })?
            .parse::<toml_edit::DocumentMut>()?;
            document["models"] = serialized["models"].clone();
        }
        if source_has_choice || choice_changed || !inline {
            if let Some(name) = &config.active_provider {
                document["model"] = toml_edit::value(name.clone());
            } else {
                document.remove("model");
            }
        }
    } else if source_has_choice || choice_changed || !inline {
        if let Some(name) = &config.active_provider {
            document["active_provider"] = toml_edit::value(name.clone());
        } else {
            document.remove("active_provider");
        }
    }
    Ok(())
}

fn persist_json_model_source(
    source: &mut serde_json::Map<String, Value>,
    config: &AppConfig,
    baseline: Option<&AppConfig>,
    inline: bool,
) -> Result<()> {
    let (choice_changed, presets_changed) = model_source_changed(config, baseline)?;
    let source_has_choice = source.contains_key("model") || source.contains_key("active_provider");
    let named = source.contains_key("models") || source.contains_key("model");
    let had_providers = source.contains_key("providers");
    if named || (!source_has_choice && !had_providers && (choice_changed || presets_changed)) {
        source.remove("active_provider");
        source.remove("providers");
        if presets_changed || had_providers {
            source.insert("models".into(), serde_json::to_value(&config.providers)?);
        }
        if source_has_choice || choice_changed || !inline {
            if let Some(name) = &config.active_provider {
                source.insert("model".into(), Value::String(name.clone()));
            } else {
                source.remove("model");
            }
        }
    } else if source_has_choice || choice_changed || !inline {
        if let Some(name) = &config.active_provider {
            source.insert("active_provider".into(), Value::String(name.clone()));
        } else {
            source.remove("active_provider");
        }
    }
    Ok(())
}

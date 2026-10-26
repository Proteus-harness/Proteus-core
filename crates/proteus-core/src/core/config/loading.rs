use std::{
    collections::BTreeSet,
    env,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use crate::core::core_slots::{CoreSlotSelection, core_slot_descriptor_by_id};

use super::{AppConfig, ConfigInputs};

impl AppConfig {
    pub async fn load(path: Option<&Path>) -> Result<Self> {
        Self::load_observed(path, &mut ConfigInputs::default()).await
    }

    pub(crate) async fn load_observed(
        path: Option<&Path>,
        inputs: &mut ConfigInputs,
    ) -> Result<Self> {
        if let Some(path) = path {
            let path = expand_user_path(path);
            inputs.observe(&path);
            if let Some(name) = config_name_ref(&path) {
                for candidate in named_config_candidates(name, default_config_dir().as_deref()) {
                    inputs.observe(&candidate);
                }
            }
        }
        let config_path = Self::resolve_config_path(path).await?;
        if let Some(path) = &config_path {
            inputs.observe(path);
        }
        let should_load = match (path, config_path.as_deref()) {
            (None, Some(path)) => tokio::fs::try_exists(path).await?,
            (_, Some(_)) => true,
            (_, None) => false,
        };
        let config = if should_load {
            Self::load_path(config_path.as_ref().expect("config path"), inputs).await?
        } else {
            Self::default()
        };
        let instructions_config_path = should_load.then_some(config_path.as_deref()).flatten();
        config
            .with_resolved_instructions(instructions_config_path, inputs)
            .await
    }

    pub async fn resolve_config_path(path: Option<&Path>) -> Result<Option<PathBuf>> {
        match path {
            Some(path) => Ok(Some(resolve_explicit_config_path(path).await?)),
            None => Ok(default_config_path()),
        }
    }

    pub fn named_config_destination_path(path: &Path) -> Option<PathBuf> {
        config_name_ref(path).map(|name| {
            default_config_dir()
                .map(|dir| dir.join(named_config_file(name)))
                .unwrap_or_else(|| PathBuf::from(named_config_file(name)))
        })
    }

    async fn load_path(path: &Path, inputs: &mut ConfigInputs) -> Result<Self> {
        let metadata = tokio::fs::metadata(path)
            .await
            .with_context(|| format!("failed to inspect config path {}", path.display()))?;
        let source_kind = if metadata.is_dir() { "dir" } else { "file" };
        let value = load_config_path_value(path, &mut BTreeSet::new(), inputs)?;
        let config: Self = serde_json::from_value(value).with_context(|| {
            format!(
                "failed to build config from {source_kind} {}",
                path.display()
            )
        })?;
        config.selected_model_config()?;
        config.validate_module_config_slots()?;
        config.modules.validate_hooks()?;
        config.validate_commands()?;
        Ok(config)
    }

    fn validate_module_config_slots(&self) -> Result<()> {
        for slot in self.module_config.keys() {
            if matches!(
                slot.as_str(),
                "tool" | "context_provider" | "model" | "hook"
            ) {
                continue;
            }
            match core_slot_descriptor_by_id(slot) {
                Some(descriptor) if descriptor.selection == CoreSlotSelection::ModulesConfig => {}
                Some(_) => bail!("module_config is not supported for slot {slot:?}"),
                None => bail!("unknown module_config slot {slot:?}"),
            }
        }
        Ok(())
    }

    pub fn default_user_config_path() -> Option<PathBuf> {
        default_config_path()
    }

    /// Резолвит `[[instructions]]` entries с `file` в текст. Относительные
    /// пути считаются от каталога config-файла, чтобы один относительный
    /// путь работал и в repo, и в установленном `~/.config/.../configs/`.
    async fn with_resolved_instructions(
        mut self,
        config_path: Option<&Path>,
        inputs: &mut ConfigInputs,
    ) -> Result<Self> {
        let base_dir = config_path.map(|path| {
            if path.is_dir() {
                path.to_path_buf()
            } else {
                path.parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf()
            }
        });
        for entry in &mut self.instructions {
            match (&entry.text, &entry.file) {
                (Some(_), Some(file)) => bail!(
                    "instructions entry must set either text or file, not both: {}",
                    file.display()
                ),
                (None, None) => bail!("instructions entry must set text or file"),
                (Some(_), None) => {}
                (None, Some(file)) => {
                    let mut path = expand_user_path(file);
                    if path.is_relative()
                        && let Some(base_dir) = &base_dir
                    {
                        path = base_dir.join(path);
                    }
                    inputs.observe(&path);
                    let text = tokio::fs::read_to_string(&path).await.with_context(|| {
                        format!("failed to read instructions file {}", path.display())
                    })?;
                    entry.text = Some(text);
                }
            }
        }
        Ok(self)
    }
}

fn load_config_path_value(
    path: &Path,
    stack: &mut BTreeSet<PathBuf>,
    inputs: &mut ConfigInputs,
) -> Result<Value> {
    inputs.observe(path);
    let canonical = std::fs::canonicalize(path)
        .with_context(|| format!("failed to inspect config path {}", path.display()))?;
    if !stack.insert(canonical.clone()) {
        bail!("config include cycle at {}", path.display());
    }

    let metadata = std::fs::metadata(&canonical)
        .with_context(|| format!("failed to inspect config path {}", canonical.display()))?;
    let value = if metadata.is_dir() {
        inputs.observe(&canonical);
        load_config_dir_value(&canonical, stack, inputs)?
    } else {
        inputs.observe(&canonical);
        load_config_file_value(&canonical, stack, inputs)?
    };

    stack.remove(&canonical);
    Ok(value)
}

fn load_config_dir_value(
    path: &Path,
    stack: &mut BTreeSet<PathBuf>,
    inputs: &mut ConfigInputs,
) -> Result<Value> {
    let mut entries = std::fs::read_dir(path)
        .with_context(|| format!("failed to read config dir {}", path.display()))?;
    let mut files = Vec::new();
    for entry in entries.by_ref() {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_file() && is_config_file(&entry.path()) {
            files.push(entry.path());
        }
    }
    files.sort();

    let mut merged = Value::Object(Map::new());
    let overlay = path.join(super::CONFIG_BUILDER_OVERLAY);
    for file in files.iter().filter(|file| **file != overlay) {
        let value = load_config_path_value(file, stack, inputs)?;
        merge_config_value(&mut merged, value);
    }
    if files.contains(&overlay) {
        let value = load_config_path_value(&overlay, stack, inputs)?;
        // Builder selections replace these complete managed objects, including
        // removal of optional slots and active_provider. Other fields still merge.
        let object = merged.as_object_mut().expect("config merge is an object");
        for key in ["modules", "module_config", "active_provider"] {
            object.remove(key);
        }
        merge_config_value(&mut merged, value);
    }

    Ok(merged)
}

fn load_config_file_value(
    path: &Path,
    stack: &mut BTreeSet<PathBuf>,
    inputs: &mut ConfigInputs,
) -> Result<Value> {
    let mut value = load_config_value(path)?;
    let includes = take_config_includes(&mut value)?;
    if includes.is_empty() {
        return Ok(value);
    }

    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut merged = Value::Object(Map::new());
    for include in includes {
        let include_path = resolve_config_include(base_dir, &include);
        let include_value = load_config_path_value(&include_path, stack, inputs)
            .with_context(|| format!("failed to include config {}", include_path.display()))?;
        merge_config_value(&mut merged, include_value);
    }
    merge_config_value(&mut merged, value);
    Ok(merged)
}

fn take_config_includes(value: &mut Value) -> Result<Vec<PathBuf>> {
    let Some(obj) = value.as_object_mut() else {
        return Ok(Vec::new());
    };
    let Some(include) = obj.remove("include") else {
        return Ok(Vec::new());
    };
    match include {
        Value::String(path) => Ok(vec![PathBuf::from(path)]),
        Value::Array(paths) => paths
            .into_iter()
            .map(|path| match path {
                Value::String(path) => Ok(PathBuf::from(path)),
                other => bail!("config include entries must be strings, got {other}"),
            })
            .collect(),
        other => bail!("config include must be a string or array of strings, got {other}"),
    }
}

fn resolve_config_include(base_dir: &Path, include: &Path) -> PathBuf {
    let include = expand_user_path(include);
    if include.is_absolute() {
        include
    } else {
        base_dir.join(include)
    }
}

fn default_config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("PROTEUS_CONFIG_PATH") {
        return Some(PathBuf::from(path));
    }

    if let Some(config_home) = env::var_os("PROTEUS_CONFIG_HOME") {
        return Some(PathBuf::from(config_home).join("configs/config.toml"));
    }

    if let Some(home) = env::var_os("HOME") {
        return Some(PathBuf::from(home).join(".config/Proteus-agent/configs/config.toml"));
    }

    env::var_os("XDG_CONFIG_HOME").map(|xdg_config_home| {
        PathBuf::from(xdg_config_home).join("Proteus-agent/configs/config.toml")
    })
}

pub(super) fn default_config_dir() -> Option<PathBuf> {
    default_config_path().and_then(|path| path.parent().map(Path::to_path_buf))
}

async fn resolve_explicit_config_path(path: &Path) -> Result<PathBuf> {
    let path = expand_user_path(path);
    let Some(name) = config_name_ref(&path) else {
        return Ok(path);
    };

    let config_dir = default_config_dir();
    resolve_config_name_path(name, config_dir.as_deref()).await
}

pub(super) async fn resolve_config_name_path(
    name: &str,
    config_dir: Option<&Path>,
) -> Result<PathBuf> {
    let candidates = named_config_candidates(name, config_dir);
    if candidates.is_empty() {
        bail!("config name '{name}' was not found; no config candidates were available");
    }

    for candidate in &candidates {
        if tokio::fs::try_exists(candidate).await? {
            return Ok(candidate.clone());
        }
    }

    bail!(
        "config name '{name}' was not found; looked for {}",
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub(super) fn named_config_candidates(name: &str, config_dir: Option<&Path>) -> Vec<PathBuf> {
    config_dir
        .map(|dir| vec![dir.join(named_config_file(name))])
        .unwrap_or_default()
}

fn named_config_file(name: &str) -> String {
    format!("{name}.config.toml")
}

pub(super) fn config_name_ref(path: &Path) -> Option<&str> {
    if path.is_absolute() || path.components().count() != 1 || path.extension().is_some() {
        return None;
    }
    let name = path.as_os_str().to_str()?;
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return None;
    }
    Some(name)
}

fn is_config_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("toml" | "json")
    )
}

fn load_config_value(path: &Path) -> Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;

    match path.extension().and_then(|extension| extension.to_str()) {
        Some("json") => serde_json::from_str(&content)
            .with_context(|| format!("failed to parse JSON config {}", path.display())),
        _ => {
            let value = toml::from_str::<toml::Value>(&content)
                .with_context(|| format!("failed to parse TOML config {}", path.display()))?;
            serde_json::to_value(value)
                .with_context(|| format!("failed to normalize TOML config {}", path.display()))
        }
    }
}

fn merge_config_value(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                merge_config_value(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, overlay) => {
            *base = overlay;
        }
    }
}

pub fn expand_user_path(path: impl AsRef<Path>) -> PathBuf {
    expand_user_path_with_home(path.as_ref(), env::var_os("HOME").as_deref())
}

pub(super) fn expand_user_path_with_home(path: &Path, home: Option<&std::ffi::OsStr>) -> PathBuf {
    let Some(path_str) = path.to_str() else {
        return path.to_path_buf();
    };
    let Some(home) = home else {
        return path.to_path_buf();
    };
    if path_str == "~" || path_str == "$HOME" || path_str == "${HOME}" {
        return PathBuf::from(home);
    }
    if let Some(stripped) = path_str.strip_prefix("~/") {
        return PathBuf::from(home).join(stripped);
    }
    if let Some(stripped) = path_str.strip_prefix("$HOME/") {
        return PathBuf::from(home).join(stripped);
    }
    if let Some(stripped) = path_str.strip_prefix("${HOME}/") {
        return PathBuf::from(home).join(stripped);
    }
    path.to_path_buf()
}

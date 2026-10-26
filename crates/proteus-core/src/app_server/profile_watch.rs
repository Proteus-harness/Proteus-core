//! Metadata polling invalidates semantic reloads without reparsing idle profiles.
use super::{AppServerHandle, prepare_assembly};
use crate::core::{AppConfig, ConfigInputs};
use anyhow::Result;
use std::{path::Path, time::Duration};
#[cfg(test)]
mod tests;

#[derive(Clone, PartialEq, Eq)]
struct Fingerprint {
    config: Vec<u8>,
    packages: Vec<u8>,
}

impl Fingerprint {
    fn new(config: &AppConfig, cwd: &Path) -> Result<Self> {
        Ok(Self {
            config: serde_json::to_vec(config)?,
            packages: crate::core::agent_plugins::fingerprint(&config.addons, cwd),
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
enum Input {
    Valid(Fingerprint),
    Invalid(String),
}

pub(super) async fn start(handle: &AppServerHandle) {
    let Some(path) = handle.config_path.clone() else {
        return;
    };
    let config = handle.config.read().await.clone();
    let Ok(mut applied) = Fingerprint::new(&config, &handle.cwd) else {
        return;
    };
    let owner = std::sync::Arc::downgrade(&handle.inner);
    let stop = handle.profile_stop.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(300));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut seen = None;
        let mut failed = None;
        let mut inputs: Option<ConfigInputs> = None;
        let mut pending = false;
        let mut addons = config.addons.clone();
        let mut source_seen = path.exists();
        loop {
            tokio::select! { _ = stop.cancelled() => break, _ = interval.tick() => {} }
            let Some(inner) = owner.upgrade() else { break };
            let handle = AppServerHandle { inner };
            if !source_seen && !path.exists() {
                continue;
            };
            source_seen = true;
            if !pending && inputs.as_ref().is_some_and(ConfigInputs::is_current) {
                continue;
            }
            let mut observed = ConfigInputs::watching();
            let input = match AppConfig::load_observed(Some(&path), &mut observed).await {
                Ok(config) => {
                    addons = config.addons.clone();
                    observed.observe_plugins(&addons, &handle.cwd);
                    match Fingerprint::new(&config, &handle.cwd) {
                        Ok(value) => Input::Valid(value),
                        Err(error) => Input::Invalid(format!("{error:#}")),
                    }
                }
                Err(error) => Input::Invalid(format!("{error:#}")),
            };
            observed.observe_plugins(&addons, &handle.cwd);
            inputs = Some(observed);
            // Editors may temporarily write an incomplete file. A stable failure
            // is reported once; it never replaces the last validated assembly.
            if seen.as_ref() != Some(&input) {
                seen = Some(input.clone());
                pending = true;
                continue;
            }
            pending = false;
            let expected = match input {
                Input::Invalid(error) => {
                    handle.set_profile_error(Some(error)).await;
                    continue;
                }
                Input::Valid(value) => value,
            };
            if expected == applied {
                handle.set_profile_error(None).await;
                failed = None;
                continue;
            }
            if failed.as_ref() == Some(&expected) {
                continue;
            };
            let current = handle.config.read().await.clone();
            if expected.config == serde_json::to_vec(&current).unwrap_or_default()
                && expected.packages == applied.packages
            {
                // A UI save has already published this session; other sessions
                // still detect the file and go through normal preparation.
                applied = expected;
                handle.set_profile_error(None).await;
                continue;
            }
            let result = tokio::select! {
                _ = stop.cancelled() => break,
                result = refresh(&handle,&path,&expected) => result,
            };
            match result {
                Ok(true) => {
                    applied = expected;
                    failed = None;
                }
                Ok(false) => {}
                Err(error) => {
                    failed = Some(expected);
                    handle.set_profile_error(Some(format!("{error:#}"))).await;
                }
            }
        }
    });
}

async fn refresh(handle: &AppServerHandle, path: &Path, expected: &Fingerprint) -> Result<bool> {
    let lock = super::config_builder::path_lock(path)?;
    let _guard = lock.lock_owned().await;
    let config = AppConfig::load(Some(path)).await?;
    if &Fingerprint::new(&config, &handle.cwd)? != expected {
        return Ok(false);
    };
    let assembly = prepare_assembly(&config, &handle.cwd, Some(path)).await?;
    let current = AppConfig::load(Some(path)).await?;
    if &Fingerprint::new(&current, &handle.cwd)? != expected {
        return Ok(false);
    };
    handle
        .publish_profile(config, assembly, None, || async { Ok(()) })
        .await?;
    Ok(true)
}

//! Cheap metadata polling with bounded-age semantic revalidation.
use super::{AppServerHandle, prepare_assembly};
use crate::core::{AppConfig, ConfigInputs};
use anyhow::Result;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

mod polling;
use polling::Polling;

// Provisional idle-cost/latency tradeoff; this is not a scheduler deadline.
const MAX_SEMANTIC_SCAN_AGE: Duration = Duration::from_secs(3);

fn metadata_allows_skip(
    pending: bool,
    last_scan_started: Option<Instant>,
    now: Instant,
    inputs_current: impl FnOnce() -> bool,
) -> bool {
    !pending
        && last_scan_started
            .is_some_and(|last| now.saturating_duration_since(last) < MAX_SEMANTIC_SCAN_AGE)
        && inputs_current()
}

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
    start_with_polling(handle, path, Polling::new()).await;
}

async fn start_with_polling(handle: &AppServerHandle, path: PathBuf, mut polling: Polling) {
    let config = handle.config.read().await.clone();
    let Ok(mut applied) = Fingerprint::new(&config, &handle.cwd) else {
        return;
    };
    let owner = std::sync::Arc::downgrade(&handle.inner);
    let stop = handle.profile_stop.clone();
    tokio::spawn(async move {
        let mut last_scan_started = None;
        let mut seen = None;
        let mut failed = None;
        let mut inputs: Option<ConfigInputs> = None;
        let mut pending = false;
        let mut addons = config.addons.clone();
        let mut source_seen = path.exists();
        loop {
            let mut poll = tokio::select! {
                _ = stop.cancelled() => break,
                poll = polling.next() => match poll {
                    Some(poll) => poll,
                    None => break,
                },
            };
            let Some(inner) = owner.upgrade() else { break };
            let handle = AppServerHandle { inner };
            if !source_seen && !path.exists() {
                continue;
            };
            source_seen = true;
            if metadata_allows_skip(pending, last_scan_started, poll.now, || {
                poll.inputs_current(inputs.as_ref())
            }) {
                continue;
            }
            // Only actual semantic attempts advance the age, including failures.
            // A slow attempt does not earn another full interval after finishing.
            last_scan_started = Some(poll.now);
            poll.record_scan();
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
            poll.record_refresh();
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

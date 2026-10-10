use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

const DEFAULT_PROFILE: &str = "config";

pub enum Target {
    Spawn { profile: String },
    Attach { origin: String, token: String },
}

pub struct LaunchOptions {
    pub workspace: PathBuf,
    pub target: Target,
}

impl LaunchOptions {
    pub fn from_args() -> Result<Self> {
        Self::parse(std::env::args().skip(1))
    }

    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut workspace = None;
        let mut profile = None;
        let mut origin = None;
        let mut token = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = |name: &str| {
                args.next()
                    .with_context(|| format!("{name} requires a value"))
            };
            match arg.as_str() {
                "--config" => profile = Some(value("--config")?),
                "--origin" => origin = Some(value("--origin")?),
                "--token" => token = Some(value("--token")?),
                flag if flag.starts_with("--") => bail!("unknown option {flag}"),
                _ if workspace.is_none() => workspace = Some(PathBuf::from(arg)),
                _ => bail!("unexpected argument {arg}"),
            }
        }

        let workspace = match workspace {
            Some(path) => path,
            None => std::env::current_dir().context("current directory")?,
        };
        let workspace = workspace
            .canonicalize()
            .with_context(|| format!("workspace {}", workspace.display()))?;

        let target = match (origin, token) {
            (Some(origin), Some(token)) => {
                if profile.is_some() {
                    bail!("--config cannot be combined with --origin");
                }
                Target::Attach { origin, token }
            }
            (None, None) => Target::Spawn {
                profile: profile
                    .or_else(saved_profile)
                    .unwrap_or_else(|| DEFAULT_PROFILE.to_owned()),
            },
            _ => bail!("--origin and --token must be given together"),
        };

        Ok(Self { workspace, target })
    }
}

#[derive(Deserialize)]
struct DesktopPreferences {
    config: Option<String>,
}

/// Reuses the profile last chosen in the Tauri app.
fn saved_profile() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(&home).join(".config"));
    let raw = std::fs::read(base.join("dev.proteus.agent/preferences.json")).ok()?;
    serde_json::from_slice::<DesktopPreferences>(&raw)
        .ok()?
        .config
}

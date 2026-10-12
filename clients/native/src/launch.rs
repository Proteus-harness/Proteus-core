use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

#[derive(Clone)]
pub enum Target {
    /// Without a profile the server loads its default config.
    Spawn { profile: Option<String> },
    Attach { origin: String, token: String },
}

pub struct LaunchOptions {
    pub workspace: PathBuf,
    pub target: Target,
}

/// Command line: the workspace stays unset when the picker was requested.
pub struct Launch {
    pub workspace: Option<PathBuf>,
    pub target: Target,
}

impl Launch {
    pub fn from_args() -> Result<Self> {
        Self::parse(std::env::args().skip(1))
    }

    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut workspace = None;
        let mut choose = false;
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
                "--choose-workspace" => choose = true,
                flag if flag.starts_with("--") => bail!("unknown option {flag}"),
                _ if workspace.is_none() => workspace = Some(PathBuf::from(arg)),
                _ => bail!("unexpected argument {arg}"),
            }
        }

        let workspace = match (workspace, choose) {
            (Some(_), true) => bail!("--choose-workspace cannot be combined with a workspace"),
            (_, true) => None,
            (Some(path), false) => Some(path),
            (None, false) => Some(std::env::current_dir().context("current directory")?),
        };
        let workspace = workspace
            .map(|path| {
                path.canonicalize()
                    .with_context(|| format!("workspace {}", path.display()))
            })
            .transpose()?;

        let target = match (origin, token) {
            (Some(origin), Some(token)) => {
                if profile.is_some() {
                    bail!("--config cannot be combined with --origin");
                }
                Target::Attach { origin, token }
            }
            (None, None) => Target::Spawn {
                profile: profile.map(anchor_profile),
            },
            _ => bail!("--origin and --token must be given together"),
        };

        Ok(Self { workspace, target })
    }
}

/// A relative config file path keeps naming the same file when the server
/// is started in another folder; bare profile names stay as given.
fn anchor_profile(profile: String) -> String {
    let path = Path::new(&profile);
    if path.is_relative()
        && path.components().count() > 1
        && let Ok(path) = path.canonicalize()
    {
        return path.display().to_string();
    }
    profile
}

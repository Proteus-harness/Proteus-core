//! Network synchronisation with the configured remotes; never prompts.
use std::path::Path;

use anyhow::{Result, bail, ensure};

use super::{
    command::{remote, run},
    require_repository,
    status::{Changes, changes},
};

pub(crate) fn fetch(root: &Path) -> Result<Changes> {
    require_repository(root)?;
    remote(root, &["fetch", "--quiet"])?.checked("fetch")?;
    changes(root)
}

/// `rebase: None` keeps the repository's own `pull.rebase` configuration.
pub(crate) fn pull(root: &Path, rebase: Option<bool>) -> Result<Changes> {
    require_repository(root)?;
    let mut args = vec!["pull", "--quiet", "--no-edit"];
    match rebase {
        Some(true) => args.push("--rebase"),
        Some(false) => args.push("--no-rebase"),
        None => {}
    }
    remote(root, &args)?.checked("pull")?;
    changes(root)
}

/// Pushes the current branch; `set_upstream` publishes a branch without an
/// upstream to the push remote and tracks it.
pub(crate) fn push(root: &Path, force: bool, set_upstream: bool) -> Result<Changes> {
    require_repository(root)?;
    let target;
    let mut args = vec!["push", "--quiet"];
    if force {
        args.push("--force-with-lease");
    }
    let has_upstream = run(
        root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )?
    .success;
    if set_upstream && !has_upstream {
        let branch = run(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        ensure!(branch.success, "cannot publish a detached HEAD");
        target = push_remote(root)?;
        args.extend(["--set-upstream", &target, "HEAD"]);
    }
    remote(root, &args)?.checked("push")?;
    changes(root)
}

/// `remote.pushDefault`, else `origin`, else the only configured remote.
fn push_remote(root: &Path) -> Result<String> {
    let configured = run(root, &["config", "--get", "remote.pushDefault"])?;
    if configured.success {
        return Ok(String::from_utf8_lossy(&configured.bytes).trim().to_owned());
    }
    let output = run(root, &["remote"])?.checked("remote")?;
    let text = String::from_utf8_lossy(&output.bytes);
    let remotes: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
    match remotes[..] {
        _ if remotes.contains(&"origin") => Ok("origin".to_owned()),
        [only] => Ok(only.to_owned()),
        [] => bail!("no Git remote is configured"),
        _ => bail!("several Git remotes are configured and none is named origin"),
    }
}

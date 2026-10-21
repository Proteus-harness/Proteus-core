use std::process::Command;

use anyhow::Result;

use crate::ProcessSpec;

pub(crate) fn prepare(spec: &ProcessSpec) -> Result<Command> {
    if let Some(sandbox) = &spec.sandbox {
        return crate::sandbox::prepare(spec, sandbox);
    }
    let mut command = Command::new(&spec.command);
    command.args(&spec.args);
    spec.apply_environment(&mut command)?;
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    Ok(command)
}

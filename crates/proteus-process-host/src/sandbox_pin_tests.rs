use super::*;
use std::{
    os::unix::fs::symlink,
    time::{SystemTime, UNIX_EPOCH},
};

fn fixture() -> Result<(PathBuf, ProcessSpec)> {
    let root = std::env::temp_dir().join(format!(
        "proteus-pin-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir(&root)?;
    let workspace = root.join("workspace");
    fs::create_dir(&workspace)?;
    let spec = ProcessSpec::new("/usr/bin/cat")
        .arg(workspace.join("input").to_string_lossy())
        .cwd(&workspace)
        .sandbox(ProcessSandbox {
            read_only: vec!["/usr".into(), "/lib64".into(), "/lib".into()],
            read_write: vec![workspace],
            network: false,
        });
    Ok((root, spec))
}

#[test]
fn pinned_mount_survives_ancestor_symlink_replacement() -> Result<()> {
    let (root, spec) = fixture()?;
    let workspace = root.join("workspace");
    fs::write(workspace.join("input"), "explicitly granted")?;
    let outside = root.join("outside");
    fs::create_dir(&outside)?;
    fs::write(outside.join("input"), "SYNTHETIC SECRET")?;
    let mut command = prepare(&spec, spec.sandbox.as_ref().unwrap())?;
    fs::rename(&workspace, root.join("original-pinned"))?;
    symlink(&outside, &workspace)?;
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(output.stdout == b"explicitly granted");
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn removed_pinned_mount_fails_without_running_payload() -> Result<()> {
    let (root, mut spec) = fixture()?;
    let source = root.join("source");
    fs::write(&source, "granted")?;
    spec.sandbox
        .as_mut()
        .unwrap()
        .read_only
        .push(source.clone());
    let marker = root.join("workspace/marker");
    spec.command = "/usr/bin/touch".into();
    spec.args = vec![marker.to_string_lossy().into()];
    let mut command = prepare(&spec, spec.sandbox.as_ref().unwrap())?;
    fs::remove_file(source)?;
    let output = command.output()?;
    ensure!(
        !output.status.success(),
        "removed mount unexpectedly succeeded"
    );
    ensure!(!marker.exists(), "payload ran after failed mount");
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn writable_hard_link_alias_is_rejected() -> Result<()> {
    let (root, mut spec) = fixture()?;
    let first = root.join("first");
    let second = root.join("second");
    fs::write(&first, "same inode")?;
    fs::hard_link(&first, &second)?;
    let sandbox = spec.sandbox.as_mut().unwrap();
    sandbox.read_only.push(first);
    sandbox.read_write.push(second);
    ensure!(spec.validate_launch().is_err());
    fs::remove_dir_all(root)?;
    Ok(())
}

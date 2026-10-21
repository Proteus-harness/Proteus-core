use super::*;
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture {
    root: PathBuf,
    workspace: PathBuf,
    bin: PathBuf,
}
impl Fixture {
    fn new() -> Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "proteus-executable-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        let workspace = root.join("workspace");
        let bin = root.join("bin");
        fs::create_dir_all(&workspace)?;
        fs::create_dir(&bin)?;
        fs::copy("/usr/bin/cat", bin.join("payload"))?;
        fs::write(bin.join("neighbor-secret"), "SYNTHETIC SECRET")?;
        fs::write(workspace.join("input"), "selected executable")?;
        Ok(Self {
            root,
            workspace,
            bin,
        })
    }
    fn spec(&self) -> ProcessSpec {
        ProcessSpec::new(self.bin.join("payload").to_string_lossy())
            .arg("input")
            .cwd(&self.workspace)
            .sandbox(ProcessSandbox {
                read_executable: true,
                read_only: vec!["/usr/lib64".into(), "/lib64".into(), "/lib".into()],
                read_write: vec![self.workspace.clone()],
                network: false,
            })
    }
    fn output(&self, spec: &ProcessSpec) -> Result<std::process::Output> {
        prepare(spec, spec.sandbox.as_ref().unwrap())?
            .output()
            .map_err(Into::into)
    }
    fn success(&self, spec: &ProcessSpec, expected: &[u8]) -> Result<()> {
        let output = self.output(spec)?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(
            output.stdout == expected,
            "unexpected output: {:?}",
            output.stdout
        );
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn executable_file_runs_without_granting_its_neighbor() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.success(&fixture.spec(), b"selected executable")?;
    let mut denied = fixture.spec();
    denied.args = vec![fixture.bin.join("neighbor-secret").to_string_lossy().into()];
    let output = fixture.output(&denied)?;
    ensure!(!output.status.success());
    ensure!(output.stdout.is_empty(), "neighbor secret leaked");
    Ok(())
}

#[test]
fn child_path_override_is_used_instead_of_parent_path() -> Result<()> {
    let fixture = Fixture::new()?;
    fs::copy("/usr/bin/printf", fixture.bin.join("cat"))?;
    let mut spec = fixture.spec().env("PATH", fixture.bin.to_string_lossy());
    spec.command = "cat".into();
    spec.args = vec!["child PATH wins".into()];
    fixture.success(&spec, b"child PATH wins")
}

#[test]
fn relative_empty_and_dot_command_paths_follow_child_cwd() -> Result<()> {
    let fixture = Fixture::new()?;
    // Cwd must be reachable, but is read-only so it cannot mutate the command.
    let cwd = fixture.root.join("readonly-cwd");
    fs::create_dir(&cwd)?;
    fs::create_dir(cwd.join("tools"))?;
    fs::copy("/usr/bin/printf", cwd.join("tools/payload"))?;
    fs::copy("/usr/bin/printf", cwd.join("payload"))?;
    for (command, path) in [
        ("payload", "tools"),
        ("payload", ""),
        ("./payload", "/absent"),
        ("../bin/payload", "/absent"),
    ] {
        let mut spec = fixture.spec().cwd(&cwd).env("PATH", path);
        spec.command = command.into();
        spec.args = if command.starts_with("../") {
            vec![fixture.workspace.join("input").to_string_lossy().into()]
        } else {
            vec!["child cwd".into()]
        };
        spec.sandbox.as_mut().unwrap().read_only.push(cwd.clone());
        fixture.success(
            &spec,
            if command.starts_with("../") {
                b"selected executable"
            } else {
                b"child cwd"
            },
        )?;
    }
    Ok(())
}

#[test]
fn path_search_skips_non_executable_regular_file_and_directory() -> Result<()> {
    let fixture = Fixture::new()?;
    let first = fixture.root.join("first");
    let second = fixture.root.join("second");
    fs::create_dir(&first)?;
    fs::create_dir_all(second.join("payload"))?;
    fs::write(first.join("payload"), "not executable")?;
    fs::set_permissions(first.join("payload"), fs::Permissions::from_mode(0o600))?;
    let path = std::env::join_paths([first, second, fixture.bin.clone()])?;
    let mut spec = fixture.spec().env("PATH", path.to_string_lossy());
    spec.command = "payload".into();
    fixture.success(&spec, b"selected executable")
}

#[test]
fn missing_command_and_missing_child_path_are_rejected() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut spec = fixture.spec();
    spec.command = "payload".into();
    spec.env_allowlist.clear();
    let error = prepare(&spec, spec.sandbox.as_ref().unwrap()).unwrap_err();
    ensure!(error.to_string().contains("requires child PATH"));
    spec.env.insert("PATH".into(), "/absent".into());
    ensure!(prepare(&spec, spec.sandbox.as_ref().unwrap()).is_err());
    spec.command = fixture.bin.join("absent").to_string_lossy().into();
    ensure!(prepare(&spec, spec.sandbox.as_ref().unwrap()).is_err());
    ensure!(!fixture.workspace.join("marker").exists());
    Ok(())
}

#[test]
fn missing_runtime_or_shebang_interpreter_never_runs_marker() -> Result<()> {
    let fixture = Fixture::new()?;
    let marker = fixture.workspace.join("marker");
    fs::copy("/usr/bin/touch", fixture.bin.join("touch"))?;
    let mut spec = fixture.spec();
    spec.command = fixture.bin.join("touch").to_string_lossy().into();
    spec.args = vec![marker.to_string_lossy().into()];
    spec.sandbox.as_mut().unwrap().read_only.clear();
    ensure!(!fixture.output(&spec)?.status.success());
    ensure!(!marker.exists());
    let script = fixture.bin.join("script");
    fs::write(
        &script,
        format!(
            "#!/usr/bin/python3\nopen({:?}, 'w').write('ran')\n",
            marker.to_string_lossy()
        ),
    )?;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700))?;
    spec.command = script.to_string_lossy().into();
    // Libraries are mounted, but granting the script never grants Python.
    spec.sandbox.as_mut().unwrap().read_only = fixture.spec().sandbox.unwrap().read_only;
    ensure!(!fixture.output(&spec)?.status.success());
    ensure!(!marker.exists());
    Ok(())
}

#[test]
fn writable_executable_tree_source_alias_and_hardlink_are_rejected() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut inside = fixture.spec();
    fs::copy("/usr/bin/cat", fixture.workspace.join("payload"))?;
    inside.command = fixture.workspace.join("payload").to_string_lossy().into();
    ensure!(prepare(&inside, inside.sandbox.as_ref().unwrap()).is_err());
    let mut tree = fixture.spec();
    tree.sandbox
        .as_mut()
        .unwrap()
        .read_write
        .push(fixture.bin.clone());
    ensure!(prepare(&tree, tree.sandbox.as_ref().unwrap()).is_err());
    let alias = fixture.root.join("writable-alias");
    symlink(&fixture.bin, &alias)?;
    let mut source = fixture.spec();
    source.sandbox.as_mut().unwrap().read_write.push(alias);
    ensure!(prepare(&source, source.sandbox.as_ref().unwrap()).is_err());
    let link = fixture.root.join("writable-hardlink");
    fs::hard_link(fixture.bin.join("payload"), &link)?;
    let mut hardlink = fixture.spec();
    hardlink.sandbox.as_mut().unwrap().read_write.push(link);
    ensure!(prepare(&hardlink, hardlink.sandbox.as_ref().unwrap()).is_err());
    Ok(())
}

#[test]
fn explicit_readonly_parent_and_duplicate_file_are_supported() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut parent = fixture.spec();
    parent
        .sandbox
        .as_mut()
        .unwrap()
        .read_only
        .push(fixture.bin.clone());
    fixture.success(&parent, b"selected executable")?;
    let mut system_parent = fixture.spec();
    system_parent.command = "/usr/bin/cat".into();
    system_parent.sandbox.as_mut().unwrap().read_only =
        vec!["/usr".into(), "/lib64".into(), "/lib".into()];
    fixture.success(&system_parent, b"selected executable")?;
    let mut duplicate = fixture.spec();
    duplicate
        .sandbox
        .as_mut()
        .unwrap()
        .read_only
        .push(fixture.bin.join("payload"));
    fixture.success(&duplicate, b"selected executable")?;
    // The general rule remains strict: only the generated file can overlap.
    parent
        .sandbox
        .as_mut()
        .unwrap()
        .read_only
        .push(fixture.bin.join("payload"));
    ensure!(prepare(&parent, parent.sandbox.as_ref().unwrap()).is_err());
    Ok(())
}

#[test]
fn prepared_executable_survives_file_symlink_and_ancestor_replacement() -> Result<()> {
    for replacement in ["file", "symlink", "ancestor"] {
        let fixture = Fixture::new()?;
        let spec = fixture.spec();
        let mut command = prepare(&spec, spec.sandbox.as_ref().unwrap())?;
        match replacement {
            "file" => {
                fs::rename(fixture.bin.join("payload"), fixture.bin.join("original"))?;
                fs::copy("/usr/bin/false", fixture.bin.join("payload"))?;
            }
            "symlink" => {
                fs::rename(fixture.bin.join("payload"), fixture.bin.join("original"))?;
                symlink("/usr/bin/false", fixture.bin.join("payload"))?;
            }
            _ => {
                fs::rename(&fixture.bin, fixture.root.join("original-bin"))?;
                let replacement = fixture.root.join("replacement");
                fs::create_dir(&replacement)?;
                fs::copy("/usr/bin/false", replacement.join("payload"))?;
                symlink(replacement, &fixture.bin)?;
            }
        }
        let output = command.output()?;
        ensure!(
            output.status.success(),
            "{replacement}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(
            output.stdout == b"selected executable",
            "{replacement} selected another command"
        );
    }
    Ok(())
}

#[test]
fn symlink_command_is_pinned_before_the_link_is_retargeted() -> Result<()> {
    let fixture = Fixture::new()?;
    let link = fixture.bin.join("link");
    symlink("payload", &link)?;
    let mut spec = fixture.spec();
    spec.command = link.to_string_lossy().into();
    let mut command = prepare(&spec, spec.sandbox.as_ref().unwrap())?;
    fs::remove_file(&link)?;
    symlink("/usr/bin/false", &link)?;
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(output.stdout == b"selected executable");
    Ok(())
}

#[test]
fn prepared_command_never_searches_changed_child_path_again() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut spec = fixture.spec().env("PATH", fixture.bin.to_string_lossy());
    spec.command = "payload".into();
    let mut command = prepare(&spec, spec.sandbox.as_ref().unwrap())?;
    // Replacing the selected PATH directory is covered by the pinned inode.
    fs::rename(&fixture.bin, fixture.root.join("old-bin"))?;
    fs::create_dir(&fixture.bin)?;
    fs::copy("/usr/bin/false", fixture.bin.join("payload"))?;
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(output.stdout == b"selected executable");
    Ok(())
}

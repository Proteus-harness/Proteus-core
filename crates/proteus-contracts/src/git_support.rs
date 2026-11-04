//! Read-only Git worktree operations must not execute configured filters.
//! Metadata commands below do not convert file content; they resolve Git's
//! own attribute precedence before a status/diff is allowed to read it.
use std::{
    collections::HashSet,
    io::{self, Read, Write},
    process::{Command, Stdio},
    thread,
};

const CONFIG_BYTES: usize = 1024 * 1024;
const PATH_BYTES: usize = 8 * 1024 * 1024;
const ATTRIBUTE_BYTES: usize = 16 * 1024 * 1024;

/// Call before adding the content-reading subcommand. The command's cwd and
/// environment define the same effective repository/config for the preflight.
/// Configured filters that match no workspace file are harmless and supported.
pub fn protect_readonly_git(command: &mut Command) -> io::Result<()> {
    command.args([
        "-c",
        "core.fsmonitor=false",
        "-c",
        "diff.external=",
        "-c",
        "diff.trustExitCode=false",
    ]);
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
    ] {
        command.env_remove(name);
    }
    command.env("GIT_OPTIONAL_LOCKS", "0");
    let mut config = metadata_command(command);
    config.args([
        "config",
        "--null",
        "--get-regexp",
        r"^filter\..*\.(clean|process)$",
    ]);
    let (success, bytes) = metadata(config, None, CONFIG_BYTES)?;
    if !success && bytes.is_empty() {
        // Exit 1 means there are no matching configuration keys. Other exit
        // statuses are rejected by `metadata`.
        return Ok(());
    }
    let mut drivers = HashSet::new();
    for record in bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let newline = record
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(|| io::Error::other("invalid Git filter configuration record"))?;
        let (key, value) = (&record[..newline], &record[newline + 1..]);
        if value.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let key = std::str::from_utf8(key).map_err(io::Error::other)?;
        let driver = key
            .strip_prefix("filter.")
            .and_then(|key| {
                key.strip_suffix(".clean")
                    .or_else(|| key.strip_suffix(".process"))
            })
            .ok_or_else(|| io::Error::other("invalid Git filter configuration key"))?;
        drivers.insert(driver.to_owned());
    }
    if drivers.is_empty() {
        return Ok(());
    }
    // Attributes may change after the preflight. Disable every discovered
    // executable driver on the final invocation as well; matched paths still
    // fail visibly below rather than returning a misleading unfiltered diff.
    for driver in &drivers {
        command
            .arg("-c")
            .arg(format!("filter.{driver}.clean="))
            .arg("-c")
            .arg(format!("filter.{driver}.process="))
            .arg("-c")
            .arg(format!("filter.{driver}.required=false"));
    }
    // A status/diff invoked from a subdirectory can inspect files elsewhere
    // in the worktree. Inspect attributes from Git's root, not just cwd.
    let mut root = metadata_command(command);
    root.args(["rev-parse", "--show-cdup"]);
    let (success, root) = metadata(root, None, 16 * 1024)?;
    if !success {
        return Err(io::Error::other("cannot resolve Git filter worktree"));
    }
    let root = std::str::from_utf8(&root)
        .map_err(io::Error::other)?
        .strip_suffix('\n')
        .ok_or_else(|| io::Error::other("invalid Git worktree prefix"))?;
    let root = if root.is_empty() { "." } else { root };
    let mut files = metadata_command(command);
    files.args(["-C", root]);
    files.args([
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
        "--deduplicate",
        "--",
        ".",
    ]);
    let (success, paths) = metadata(files, None, PATH_BYTES)?;
    if !success {
        return Err(io::Error::other("cannot inspect Git filter paths"));
    }
    if paths.is_empty() {
        return Ok(());
    }
    let mut attributes = metadata_command(command);
    attributes.args(["-C", root]);
    attributes.args(["check-attr", "-z", "--stdin", "filter"]);
    let (success, bytes) = metadata(attributes, Some(paths), ATTRIBUTE_BYTES)?;
    if !success {
        return Err(io::Error::other("cannot inspect Git filter attributes"));
    }
    let mut records = bytes.split(|byte| *byte == 0);
    while let Some(path) = records.next().filter(|path| !path.is_empty()) {
        let attribute = records
            .next()
            .ok_or_else(|| io::Error::other("incomplete Git attribute"))?;
        let driver = records
            .next()
            .ok_or_else(|| io::Error::other("incomplete Git attribute"))?;
        if attribute != b"filter" {
            return Err(io::Error::other("unexpected Git attribute"));
        }
        if let Ok(driver) = std::str::from_utf8(driver)
            && drivers.contains(driver)
        {
            return Err(io::Error::other(format!(
                "read-only Git refuses executable filter '{driver}' for '{}'; configured filters require a separate write-capable Git operation",
                String::from_utf8_lossy(path),
            )));
        }
    }
    Ok(())
}

fn metadata_command(source: &Command) -> Command {
    let mut command = Command::new(source.get_program());
    command.args(source.get_args());
    if let Some(cwd) = source.get_current_dir() {
        command.current_dir(cwd);
    }
    for (key, value) in source.get_envs() {
        if let Some(value) = value {
            command.env(key, value);
        } else {
            command.env_remove(key);
        }
    }
    command
}

fn metadata(
    mut command: Command,
    input: Option<Vec<u8>>,
    limit: usize,
) -> io::Result<(bool, Vec<u8>)> {
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let input = input.map(|bytes| {
        let mut stdin = child.stdin.take().expect("piped metadata input");
        thread::spawn(move || stdin.write_all(&bytes))
    });
    let mut stderr = child.stderr.take().expect("piped metadata error");
    let errors = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = (&mut stderr).take(4096).read_to_end(&mut bytes);
        let _ = io::copy(&mut stderr, &mut io::sink());
        bytes
    });
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .expect("piped metadata output")
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes);
    if bytes.len() > limit || read.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    if let Some(input) = input {
        let _ = input.join();
    }
    let stderr = errors.join().unwrap_or_default();
    read?;
    if bytes.len() > limit {
        return Err(io::Error::other(
            "Git filter preflight exceeds its byte budget",
        ));
    }
    if !status.success() && status.code() != Some(1) {
        return Err(io::Error::other(format!(
            "cannot inspect Git filters: {}",
            String::from_utf8_lossy(&stderr).trim()
        )));
    }
    Ok((status.success(), bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn git(root: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn clean_and_process_filters_are_blocked_from_local_and_global_configs() {
        for key in ["clean", "process"] {
            for global in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let root = directory.path();
                git(root, &["init", "-q"]);
                std::fs::write(root.join("tracked"), "before\n").unwrap();
                git(root, &["add", "tracked"]);
                std::fs::write(root.join("tracked"), "changed\n").unwrap();
                std::fs::write(root.join(".gitattributes"), "tracked filter=marker\n").unwrap();
                let marker = root.join("unexpected-marker");
                let script = root.join("filter.sh");
                std::fs::write(&script, format!("touch '{}'\ncat\n", marker.display())).unwrap();
                let config = root.join("global.config");
                if global {
                    git(
                        root,
                        &[
                            "config",
                            "--file",
                            config.to_str().unwrap(),
                            &format!("filter.marker.{key}"),
                            &format!("sh '{}'", script.display()),
                        ],
                    );
                } else {
                    git(
                        root,
                        &[
                            "config",
                            &format!("filter.marker.{key}"),
                            &format!("sh '{}'", script.display()),
                        ],
                    );
                }
                let mut command = Command::new("git");
                command
                    .current_dir(root)
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env(
                        "GIT_CONFIG_GLOBAL",
                        if global {
                            config.as_path()
                        } else {
                            Path::new("/dev/null")
                        },
                    );
                let error = protect_readonly_git(&mut command).unwrap_err();
                assert!(
                    error.to_string().contains("executable filter 'marker'"),
                    "{error}"
                );
                assert!(!marker.exists());
            }
        }
    }

    #[test]
    fn nested_cwd_inspects_filters_elsewhere_in_the_worktree() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-q"]);
        std::fs::create_dir(root.join("nested")).unwrap();
        std::fs::write(root.join("file"), "contents").unwrap();
        std::fs::write(root.join(".gitattributes"), "file filter=marker\n").unwrap();
        git(root, &["config", "filter.marker.clean", "false"]);
        let mut command = Command::new("git");
        command
            .current_dir(root.join("nested"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        assert!(
            protect_readonly_git(&mut command)
                .unwrap_err()
                .to_string()
                .contains("executable filter 'marker'")
        );
    }

    #[test]
    fn unknown_driver_and_unused_executable_driver_do_not_break_readonly_git() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-q"]);
        std::fs::write(root.join("file"), "contents").unwrap();
        std::fs::write(root.join(".gitattributes"), "file filter=not-configured\n").unwrap();
        git(root, &["config", "filter.unused.clean", "false"]);
        let mut command = Command::new("git");
        command
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        protect_readonly_git(&mut command).unwrap();
        assert!(
            command
                .args(["status", "--short"])
                .status()
                .unwrap()
                .success()
        );
    }
}

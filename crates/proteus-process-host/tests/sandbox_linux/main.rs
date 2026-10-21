use std::{
    fs::{self, File},
    io::{self, BufRead, Write},
    net::{TcpListener, TcpStream},
    os::{fd::AsRawFd, unix::fs::symlink},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use proteus_process_host::{
    NewlineJsonFraming, ProcessHost, ProcessSandbox, ProcessSpec, ProcessTransport,
};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);

pub fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("__sandbox_fixture") {
        return fixture(&args[2..]);
    }
    let root = std::env::temp_dir().join(format!("proteus-host-sandbox-{}", std::process::id()));
    fs::create_dir(&root)?;
    let result = tests(&root);
    fs::remove_dir_all(&root)?;
    result
}

fn base(workspace: &Path) -> Result<ProcessSpec> {
    let executable = std::env::current_exe()?;
    Ok(ProcessSpec::new(executable.to_string_lossy())
        .arg("__sandbox_fixture")
        .cwd(workspace)
        .sandbox(ProcessSandbox {
            read_only: vec![
                "/usr".into(),
                "/lib64".into(),
                "/lib".into(),
                "/bin".into(),
                executable,
            ],
            read_write: vec![workspace.into()],
            network: false,
        }))
}

fn response(spec: &ProcessSpec) -> Result<Value> {
    let mut process = ProcessTransport::spawn(spec, NewlineJsonFraming::default())?;
    let frame = process.recv_frame(WAIT)?;
    ensure!(
        process.lifecycle().wait_for_exit(WAIT)?.is_some(),
        "payload did not exit"
    );
    process.terminate()?;
    Ok(frame)
}

fn tests(root: &Path) -> Result<()> {
    let workspace = root.join("workspace");
    let outside = root.join("outside");
    fs::create_dir(&workspace)?;
    fs::create_dir(&outside)?;
    let secret = outside.join("secret");
    fs::write(&secret, "SYNTHETIC SECRET")?;
    fs::write(workspace.join("allowed"), "allowed")?;
    let readonly = root.join("read only fixture");
    fs::write(&readonly, "read only")?;
    symlink(&secret, workspace.join("secret-link"))?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?.to_string();
    let fd = File::open(&secret)?;
    ensure!(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, 0) } == 0);
    let mut spec = base(&workspace)?
        .arg("probe")
        .arg(secret.to_string_lossy())
        .arg(outside.join("forbidden").to_string_lossy())
        .arg(&address)
        .arg(readonly.to_string_lossy())
        .env("SANDBOX_EXPLICIT", "visible")
        .env("LD_PRELOAD", "/absent-proteus-test.so");
    spec.sandbox
        .as_mut()
        .unwrap()
        .read_only
        .push(readonly.clone());
    spec.validate_launch()?;
    for language in ["rust", "python"] {
        let probe = if language == "python" {
            let mut python = spec.clone();
            python.command = "/usr/bin/python3".into();
            python.args = vec![
                "-c".into(),
                include_str!("probe.py").into(),
                secret.to_string_lossy().into(),
                outside.join("forbidden").to_string_lossy().into(),
                address.clone(),
                readonly.to_string_lossy().into(),
            ];
            python
        } else {
            spec.clone()
        };
        let frame = response(&probe).with_context(|| format!("{language} restricted probe"))?;
        ensure!(
            frame["read"] == "allowed" && frame["write"] == true,
            "{language}: {frame}"
        );
        for key in [
            "secret_denied",
            "symlink_denied",
            "write_denied",
            "root_write_denied",
            "readonly_write_denied",
            "network_denied",
            "env_clean",
            "fds_clean",
        ] {
            ensure!(frame[key] == true, "{language} failed {key}: {frame}");
        }
        ensure!(
            frame["readonly_read"] == "read only",
            "explicit read-only grant was unavailable"
        );
        ensure!(
            frame["path"] == std::env::var("PATH")?,
            "PATH allowlist was not preserved"
        );
        ensure!(!outside.join("forbidden").exists());
        let mut shared_net = probe;
        shared_net.sandbox.as_mut().unwrap().network = true;
        ensure!(
            response(&shared_net)?["network_denied"] == false,
            "{language} explicit networking denied"
        );
        println!("PASS {language}: filesystem, symlink, network, environment and FD isolation");
    }
    // Restore the parent's flag before opening more descriptors.
    ensure!(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == 0);
    for mode in ["terminate", "drop", "natural"] {
        teardown(&base(&workspace)?, &workspace, mode)?;
        println!("PASS {mode}: setsid descendant holding stdout terminated");
    }
    reset(&base(&workspace)?, &workspace)?;
    println!("PASS reset: setsid descendant terminated and fresh generation started");
    invalid_grants(&base(&workspace)?, root, &workspace)?;
    backend_failure(&base(&workspace)?, &workspace)?;
    println!("PASS fail-closed validation and mount/backend failure");
    Ok(())
}

fn invalid_grants(spec: &ProcessSpec, root: &Path, workspace: &Path) -> Result<()> {
    for paths in [
        vec![PathBuf::from("/")],
        vec!["/proc".into()],
        vec!["/dev/null".into()],
        vec!["/sys".into()],
        vec!["/run".into()],
        vec!["/tmp".into()],
        vec![root.join("missing")],
        vec!["relative".into()],
        vec![workspace.into(), workspace.join("allowed")],
    ] {
        let mut invalid = spec.clone();
        invalid.sandbox.as_mut().unwrap().read_only = paths;
        let error = invalid
            .validate_launch()
            .expect_err("invalid sandbox was accepted");
        ensure!(!error.to_string().is_empty());
    }
    let mut invalid = spec.clone();
    invalid
        .sandbox
        .as_mut()
        .unwrap()
        .read_write
        .push("/lib64".into());
    ensure!(
        invalid.validate_launch().is_err(),
        "writable source alias accepted"
    );
    let mut invalid = spec.clone();
    invalid.cwd = Some(root.into());
    ensure!(invalid.validate_launch().is_err(), "ungranted cwd accepted");
    // A failed mount source must never execute the marker, even when the
    // unconfined command would have access to the same host directory.
    let marker = workspace.join("unsandboxed-marker");
    let mut invalid = spec.clone().arg("marker").arg(marker.to_string_lossy());
    invalid
        .sandbox
        .as_mut()
        .unwrap()
        .read_only
        .push(root.join("missing"));
    ensure!(ProcessTransport::spawn(&invalid, NewlineJsonFraming::default()).is_err());
    ensure!(!marker.exists());
    Ok(())
}

fn backend_failure(spec: &ProcessSpec, workspace: &Path) -> Result<()> {
    // Backend is present but the requested payload is deliberately not mounted.
    // An unconfined fallback would execute our existing fixture and write it.
    let executable = std::env::current_exe()?;
    let marker = workspace.join("backend-fallback-marker");
    let mut missing = spec.clone().arg("marker").arg(marker.to_string_lossy());
    missing
        .sandbox
        .as_mut()
        .unwrap()
        .read_only
        .retain(|path| path != &executable);
    let mut process = ProcessTransport::spawn(&missing, NewlineJsonFraming::default())?;
    let status = process
        .lifecycle()
        .wait_for_exit(WAIT)?
        .context("backend did not fail")?;
    ensure!(!status.success(), "unmounted payload unexpectedly ran");
    process.terminate()?;
    ensure!(!marker.exists());

    // Hide only the backend inside a disposable outer namespace. The host
    // /usr/bin/bwrap is never moved or modified, and the Rust payload retains
    // its loader/libraries plus a writable synthetic marker directory.
    let marker = workspace.join("missing-backend-marker");
    let mut outer = spec
        .clone()
        .arg("missing_backend")
        .arg(marker.to_string_lossy());
    outer.sandbox.as_mut().unwrap().read_only = vec![
        "/usr/lib64".into(),
        "/lib64".into(),
        "/lib".into(),
        executable,
    ];
    let frame = response(&outer)?;
    ensure!(
        frame["backend_absent"] == true && frame["spawn_rejected"] == true,
        "missing backend was not rejected: {frame}"
    );
    ensure!(
        frame["marker_absent"] == true && !marker.exists(),
        "payload ran without the sandbox backend: {frame}"
    );
    ensure!(
        frame["error"]
            .as_str()
            .unwrap_or_default()
            .contains("restricted sandbox requires /usr/bin/bwrap"),
        "wrong failure: {frame}"
    );
    Ok(())
}

fn teardown(spec: &ProcessSpec, workspace: &Path, mode: &str) -> Result<()> {
    let heartbeat = workspace.join(format!("heartbeat-{mode}"));
    let mut process = ProcessTransport::spawn(
        &spec
            .clone()
            .arg("descendant")
            .arg(heartbeat.to_string_lossy())
            .arg(mode),
        NewlineJsonFraming::default(),
    )?;
    ensure!(process.recv_frame(WAIT)?["ready"] == true);
    let started = Instant::now();
    if mode == "natural" {
        ensure!(
            process.lifecycle().wait_for_exit(WAIT)?.is_some(),
            "natural exit stuck"
        );
        process.terminate()?;
    } else if mode == "drop" {
        drop(process);
    } else {
        process.terminate()?;
    }
    ensure!(
        started.elapsed() < Duration::from_secs(5),
        "generation shutdown blocked"
    );
    let stamp = fs::read(&heartbeat)?;
    std::thread::sleep(Duration::from_millis(150));
    ensure!(
        fs::read(&heartbeat)? == stamp,
        "escaped descendant still running"
    );
    Ok(())
}

fn reset(spec: &ProcessSpec, workspace: &Path) -> Result<()> {
    let heartbeat = workspace.join("heartbeat-reset");
    let host = ProcessHost::new(
        spec.clone()
            .arg("descendant")
            .arg(heartbeat.to_string_lossy())
            .arg("reset"),
        NewlineJsonFraming::default(),
    );
    ensure!(host.recv_frame(WAIT)?["ready"] == true);
    let old = host.ensure_session()?.lifecycle();
    host.reset();
    let stamp = fs::read(&heartbeat)?;
    std::thread::sleep(Duration::from_millis(150));
    ensure!(
        fs::read(&heartbeat)? == stamp,
        "reset left descendant alive"
    );
    fs::remove_file(&heartbeat)?;
    ensure!(host.recv_frame(WAIT)?["ready"] == true);
    let fresh = host.ensure_session()?.lifecycle();
    ensure!(!old.same_generation(&fresh), "reset reused generation");
    host.terminate()?;
    Ok(())
}

fn fixture(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("probe") => {
            let mut socket = false;
            if TcpStream::connect_timeout(&args[3].parse()?, Duration::from_millis(200)).is_ok() {
                socket = true;
            }
            let fds_clean = fs::read_dir("/proc/self/fd")?
                .filter_map(Result::ok)
                .all(|entry| {
                    let Ok(number) = entry.file_name().to_string_lossy().parse::<i32>() else {
                        return false;
                    };
                    number < 3
                        || fs::read_link(entry.path()).is_err()
                        || fs::read_link(entry.path())
                            .unwrap()
                            .to_string_lossy()
                            .starts_with("/proc/")
                });
            fs::write("written", "yes")?;
            println!(
                "{}",
                json!({
                    "read": fs::read_to_string("allowed")?, "write": fs::read_to_string("written")? == "yes",
                    "secret_denied": fs::read(&args[1]).is_err(), "symlink_denied": fs::read("secret-link").is_err(),
                    "write_denied": fs::write(&args[2], "forbidden").is_err(),
                "readonly_read": fs::read_to_string(&args[4])?,
                "readonly_write_denied": fs::write(&args[4], "forbidden").is_err(),
                "root_write_denied": fs::write("/proteus-outside-grant", "forbidden").is_err(),
                "path": std::env::var("PATH")?, "network_denied": !socket,
                    "env_clean": std::env::var("HOME").is_err() && std::env::var("SANDBOX_EXPLICIT")? == "visible" && std::env::var("LD_PRELOAD")? == "/absent-proteus-test.so",
                    "fds_clean": fds_clean,
                })
            );
        }
        Some("missing_backend") => {
            let backend_absent = !Path::new("/usr/bin/bwrap").exists();
            let inner = ProcessSpec::new(std::env::current_exe()?.to_string_lossy())
                .args(["__sandbox_fixture", "marker", &args[1]])
                .cwd(std::env::current_dir()?)
                .sandbox(ProcessSandbox {
                    read_only: vec![std::env::current_exe()?],
                    read_write: vec![std::env::current_dir()?],
                    network: false,
                });
            let error = ProcessTransport::spawn(&inner, NewlineJsonFraming::default())
                .expect_err("missing sandbox backend unexpectedly allowed a launch");
            println!(
                "{}",
                json!({
                    "backend_absent": backend_absent, "spawn_rejected": true,
                    "marker_absent": !Path::new(&args[1]).exists(), "error": format!("{error:#}"),
                })
            );
        }
        Some("marker") => fs::write(&args[1], "UNSANDBOXED")?,
        Some("descendant") => {
            let mut descendant = Command::new("/usr/bin/python3").args(["-c",
                "import os,time,sys; os.setsid(); f=open(sys.argv[1],'ab',buffering=0);\nwhile True: f.write(b'x'); time.sleep(0.01)", &args[1]]).spawn()?;
            while !Path::new(&args[1]).exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
            println!("{}", json!({"ready":true}));
            io::stdout().flush()?;
            if args[2] == "natural" {
                return Ok(());
            }
            // Keep the payload alive; descendant holds inherited stdout too.
            let mut line = String::new();
            io::stdin().lock().read_line(&mut line)?;
            let _ = descendant.wait();
        }
        _ => anyhow::bail!("unknown fixture mode"),
    }
    Ok(())
}

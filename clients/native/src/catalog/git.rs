//! Non-interactive system git. Only this private cache is checked out;
//! repository refresh never touches a user's working copy.

use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};

use super::{
    Source,
    schema::{self, Catalog},
};

const TIMEOUT: Duration = Duration::from_secs(90);

fn run(args: &[&str], dir: &Path) -> Result<()> {
    // A file, rather than an unread pipe, prevents verbose git from blocking.
    let log = dir.join(format!(".git-output-{}", uuid::Uuid::new_v4()));
    let output = std::fs::File::create(&log)?;
    let result = (|| {
        let mut child = Command::new("git")
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "")
            .env("SSH_ASKPASS", "")
            .env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes -oConnectTimeout=15")
            .env("GIT_CONFIG_COUNT", "2")
            .env("GIT_CONFIG_KEY_0", "http.lowSpeedLimit")
            .env("GIT_CONFIG_VALUE_0", "1000")
            .env("GIT_CONFIG_KEY_1", "http.lowSpeedTime")
            .env("GIT_CONFIG_VALUE_1", "30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(output)
            .spawn()
            .context("Не удалось запустить git")?;
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                let text = std::fs::read_to_string(&log).unwrap_or_default();
                ensure!(status.success(), "git: {}", text.trim());
                return Ok(());
            }
            if start.elapsed() >= TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                bail!("git: превышено время ожидания (90 с)");
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    })();
    let _ = std::fs::remove_file(log);
    result
}

pub fn validate(source: &Source) -> Result<()> {
    ensure!(
        !source.url.trim().is_empty() && !source.url.starts_with('-'),
        "Укажите URL репозитория"
    );
    if let Some(branch) = &source.branch {
        ensure!(
            !branch.is_empty()
                && !branch.starts_with('-')
                && !branch.chars().any(char::is_whitespace),
            "Неверная ветка"
        );
    }
    Ok(())
}

/// Reads an existing checkout without contacting its origin.
pub fn cached(cache: &Path, source: &Source) -> Result<Catalog> {
    schema::read(&cache.join(source.key()), &source.name())
}

pub fn refresh(cache: &Path, source: &Source) -> Result<Catalog> {
    // Multiple settings windows share the same on-disk checkout and git index.
    static REFRESH: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = REFRESH
        .lock()
        .map_err(|_| anyhow::anyhow!("Предыдущее обновление каталога прервано"))?;
    validate(source)?;
    std::fs::create_dir_all(cache)?;
    let repo = cache.join(format!(".repo-{}", source.key()));
    let repo_text = repo.to_str().context("Путь к кэшу не в UTF-8")?;
    if !repo.exists() {
        let mut args = vec!["clone", "--bare", "--depth=1"];
        if let Some(branch) = &source.branch {
            args.extend(["--branch", branch.as_str()]);
        }
        args.extend(["--", source.url.as_str(), repo_text]);
        if let Err(error) = run(&args, cache) {
            let _ = std::fs::remove_dir_all(&repo);
            return Err(error);
        }
    }
    let reference = source.branch.as_deref().unwrap_or("HEAD");
    run(
        &[
            "--git-dir",
            repo_text,
            "fetch",
            "--depth=1",
            "origin",
            reference,
        ],
        cache,
    )?;
    let temporary = cache.join(format!(".checkout-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&temporary)?;
    let result = (|| {
        let worktree = temporary.to_str().context("Путь к кэшу не в UTF-8")?;
        // The index belongs exclusively to this source's private bare repo.
        run(
            &[
                "--git-dir",
                repo_text,
                "--work-tree",
                worktree,
                "checkout",
                "--force",
                "FETCH_HEAD",
                "--",
                ".",
            ],
            cache,
        )?;
        schema::read(&temporary, &source.name())?;
        crate::packages::publish(&temporary, &cache.join(source.key()), true)?;
        cached(cache, source)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    result
}

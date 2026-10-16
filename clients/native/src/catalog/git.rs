//! Non-interactive system git over a private blobless clone. A refresh
//! downloads the tree and only the files that list the entries; the files
//! of an entry come on demand, when it is opened or installed. Repository
//! refresh never touches a user's working copy.

use std::{
    collections::{BTreeMap, HashMap},
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use super::{
    Source,
    schema::{self, Catalog, INDEX},
};

const TIMEOUT: Duration = Duration::from_secs(90);
/// The file that lists a repository without the index.
const SKILL: &str = "SKILL.md";

/// The commit the checkout shows and the git tree id of every folder in it,
/// which versions the folders without downloading them.
#[derive(Default, Deserialize, Serialize)]
struct State {
    commit: String,
    trees: BTreeMap<String, String>,
}

/// One object of `git ls-tree`.
struct Item {
    kind: String,
    oid: String,
    path: String,
}

fn run(args: &[&str], dir: &Path, input: Option<&[u8]>) -> Result<Vec<u8>> {
    // Files, rather than unread pipes, prevent verbose git from blocking.
    let name = uuid::Uuid::new_v4();
    let (log, out, feed) = (
        dir.join(format!(".git-output-{name}")),
        dir.join(format!(".git-stdout-{name}")),
        dir.join(format!(".git-stdin-{name}")),
    );
    let result = (|| {
        let stdin = match input {
            Some(input) => {
                std::fs::write(&feed, input)?;
                Stdio::from(std::fs::File::open(&feed)?)
            }
            None => Stdio::null(),
        };
        let mut command = Command::new("git");
        // Own group: a timeout also stops the transport helpers git starts.
        command.process_group(0);
        // SAFETY: prctl is async-signal-safe; git must not outlive the client.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "")
            .env("SSH_ASKPASS", "")
            .env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes -oConnectTimeout=15")
            // Paths are entry folders and files, never patterns.
            .env("GIT_LITERAL_PATHSPECS", "1")
            // Missing files are fetched in one request, never one by one.
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_CONFIG_COUNT", "2")
            .env("GIT_CONFIG_KEY_0", "http.lowSpeedLimit")
            .env("GIT_CONFIG_VALUE_0", "1000")
            .env("GIT_CONFIG_KEY_1", "http.lowSpeedTime")
            .env("GIT_CONFIG_VALUE_1", "30")
            .stdin(stdin)
            .stdout(std::fs::File::create(&out)?)
            .stderr(std::fs::File::create(&log)?)
            .spawn()
            .context("Не удалось запустить git")?;
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                let text = std::fs::read_to_string(&log).unwrap_or_default();
                ensure!(status.success(), "git: {}", text.trim());
                return Ok(std::fs::read(&out)?);
            }
            if start.elapsed() >= TIMEOUT {
                // SAFETY: the group is the one this child leads.
                unsafe { libc::killpg(child.id() as libc::pid_t, libc::SIGKILL) };
                let _ = child.wait();
                bail!("git: превышено время ожидания (90 с)");
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    })();
    for file in [log, out, feed] {
        let _ = std::fs::remove_file(file);
    }
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
    let state = read_state(cache, source)?;
    schema::read(&cache.join(source.key()), &source.name(), &state.trees)
}

pub fn refresh(cache: &Path, source: &Source) -> Result<Catalog> {
    let lock = lock(source)?;
    let _guard = lock
        .lock()
        .map_err(|_| anyhow::anyhow!("Предыдущее обновление каталога прервано"))?;
    validate(source)?;
    std::fs::create_dir_all(cache)?;
    let repo = repo_path(cache, source);
    let repo_text = repo.to_str().context("Путь к кэшу не в UTF-8")?;
    if !repo.exists() {
        let mut args = vec!["clone", "--bare", "--depth=1", "--filter=blob:none"];
        if let Some(branch) = &source.branch {
            args.extend(["--branch", branch.as_str()]);
        }
        args.extend(["--", source.url.as_str(), repo_text]);
        if let Err(error) = run(&args, cache, None) {
            let _ = std::fs::remove_dir_all(&repo);
            return Err(error);
        }
    }
    let reference = source.branch.as_deref().unwrap_or("HEAD");
    // A partial clone fetches with the filter it was cloned with.
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
        None,
    )?;
    let commit = text(run(
        &["--git-dir", repo_text, "rev-parse", "FETCH_HEAD^{commit}"],
        cache,
        None,
    )?)?;
    let root = text(run(
        &[
            "--git-dir",
            repo_text,
            "rev-parse",
            &format!("{commit}^{{tree}}"),
        ],
        cache,
        None,
    )?)?;
    let items = list(repo_text, &commit, &[], true, cache)?;
    let mut trees = BTreeMap::from([(String::new(), root)]);
    trees.extend(
        items
            .iter()
            .filter(|item| item.kind == "tree")
            .map(|item| (item.path.clone(), item.oid.clone())),
    );
    let indexed = items.iter().any(|item| item.path == INDEX);
    let listing = items
        .iter()
        .filter(|item| {
            item.kind == "blob"
                && if indexed {
                    item.path == INDEX
                } else {
                    item.path == SKILL || item.path.ends_with(&format!("/{SKILL}"))
                }
        })
        .collect::<Vec<_>>();
    let temporary = cache.join(format!(".checkout-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&temporary)?;
    let result = (|| {
        // Every folder exists, so entries resolve before their files arrive;
        // repository metadata folders are never part of the checkout.
        for path in trees
            .keys()
            .filter(|path| !path.is_empty() && schema::relative(Path::new(path.as_str())).is_ok())
        {
            std::fs::create_dir_all(temporary.join(path))?;
        }
        checkout(repo_text, &commit, &listing, &temporary, cache)?;
        schema::read(&temporary, &source.name(), &trees)?;
        crate::packages::publish(&temporary, &cache.join(source.key()), true)?;
        let state = State { commit, trees };
        std::fs::write(state_path(cache, source), serde_json::to_vec(&state)?)?;
        cached(cache, source)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    result
}

/// Brings files of the checkout from the origin and runs `work` with them
/// in place: each path is a file or a whole folder, missing ones are left
/// out. A refresh waits until `work` is done.
pub fn with_files<T>(
    cache: &Path,
    source: &Source,
    paths: &[PathBuf],
    work: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let lock = lock(source)?;
    let _guard = lock
        .lock()
        .map_err(|_| anyhow::anyhow!("Предыдущее обновление каталога прервано"))?;
    let state = read_state(cache, source)?;
    let repo = repo_path(cache, source);
    let repo_text = repo.to_str().context("Путь к кэшу не в UTF-8")?;
    let mut paths = paths
        .iter()
        .map(|path| {
            schema::relative(path)?;
            Ok(path
                .components()
                .filter(|part| !matches!(part, std::path::Component::CurDir))
                .collect::<PathBuf>())
        })
        .collect::<Result<Vec<_>>>()?;
    // The repository root is all of it.
    if paths.iter().any(|path| path.as_os_str().is_empty()) {
        paths.clear();
    }
    let paths = paths
        .iter()
        .map(|path| path.to_str().context("Путь не в UTF-8"))
        .collect::<Result<Vec<_>>>()?;
    let items = list(repo_text, &state.commit, &paths, false, cache)?;
    let files = items
        .iter()
        .filter(|item| item.kind == "blob")
        .collect::<Vec<_>>();
    checkout(
        repo_text,
        &state.commit,
        &files,
        &cache.join(source.key()),
        cache,
    )?;
    work()
}

fn lock(source: &Source) -> Result<Arc<Mutex<()>>> {
    // Sources download side by side; one source's checkout and git index
    // take a single writer, even one left by a closed settings screen.
    static LOCKS: LazyLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
        LazyLock::new(Default::default);
    Ok(LOCKS
        .lock()
        .map_err(|_| anyhow::anyhow!("Предыдущее обновление каталога прервано"))?
        .entry(source.key())
        .or_default()
        .clone())
}

fn repo_path(cache: &Path, source: &Source) -> PathBuf {
    cache.join(format!(".git-{}", source.key()))
}

fn state_path(cache: &Path, source: &Source) -> PathBuf {
    cache.join(format!(".state-{}.json", source.key()))
}

fn read_state(cache: &Path, source: &Source) -> Result<State> {
    let text = std::fs::read(state_path(cache, source)).context("Каталог ещё не скачан")?;
    serde_json::from_slice(&text).context("Повреждён кэш каталога")
}

fn text(output: Vec<u8>) -> Result<String> {
    Ok(String::from_utf8(output)
        .context("git вернул не UTF-8")?
        .trim()
        .to_owned())
}

/// Objects under `paths` of the commit, or all of them; `trees` adds folders.
fn list(repo: &str, commit: &str, paths: &[&str], trees: bool, dir: &Path) -> Result<Vec<Item>> {
    let mut args = vec!["--git-dir", repo, "ls-tree", "-r", "-z"];
    if trees {
        args.push("-t");
    }
    args.extend(["--", commit]);
    args.extend(paths);
    let output = run(&args, dir, None)?;
    String::from_utf8(output)
        .context("git вернул не UTF-8 путь")?
        .split('\0')
        .filter(|record| !record.is_empty())
        .map(|record| {
            let (meta, path) = record.split_once('\t').context("Неверный вывод git")?;
            let mut meta = meta.split(' ');
            let (_mode, kind, oid) = (meta.next(), meta.next(), meta.next());
            Ok(Item {
                kind: kind.context("Неверный вывод git")?.to_owned(),
                oid: oid.context("Неверный вывод git")?.to_owned(),
                path: path.to_owned(),
            })
        })
        .collect()
}

/// Writes `files` of the commit into `worktree`, fetching the missing ones
/// in one request first.
fn checkout(repo: &str, commit: &str, files: &[&Item], worktree: &Path, dir: &Path) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    let oids = files
        .iter()
        .map(|item| format!("{}\n", item.oid))
        .collect::<String>();
    let present = run(
        &["--git-dir", repo, "cat-file", "--batch-check"],
        dir,
        Some(oids.as_bytes()),
    )?;
    let missing = String::from_utf8_lossy(&present)
        .lines()
        .filter_map(|line| line.strip_suffix(" missing"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        let mut args = vec![
            "--git-dir",
            repo,
            "-c",
            "fetch.negotiationAlgorithm=noop",
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            "--recurse-submodules=no",
            "origin",
        ];
        args.extend(missing.iter().map(String::as_str));
        run(&args, dir, None)?;
    }
    let worktree = worktree.to_str().context("Путь к кэшу не в UTF-8")?;
    let paths = files
        .iter()
        .flat_map(|item| [item.path.as_bytes(), b"\0"])
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    // The index belongs exclusively to this source's private bare repo.
    run(
        &[
            "--git-dir",
            repo,
            "--work-tree",
            worktree,
            "checkout",
            "--force",
            commit,
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ],
        dir,
        Some(&paths),
    )?;
    Ok(())
}

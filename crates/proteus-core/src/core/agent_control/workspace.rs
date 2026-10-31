//! Git worktree-workspace, принадлежащий lifecycle Agent Control.
//!
//! Lifecycle оркестрирует policy-gated facade-tool `task`: после approval он
//! создаёт worktree, подменяет `task.cwd` ребёнка и после завершения просит
//! cleanup. Здесь — только sync-механика поверх системного `git`; async tool
//! вызывает её через `spawn_blocking`.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use crate::contracts::WorkspaceInfo;

/// Каталог worktree-ов относительно repo root. Добавляется в
/// `.git/info/exclude`, а не в `.gitignore` репозитория.
const WORKTREES_DIR: &str = ".proteus/worktrees";
const EXCLUDE_LINE: &str = "/.proteus/";

/// Создаёт worktree `<repo_root>/.proteus/worktrees/<name>` на новой ветке
/// `proteus/<name>` от текущего HEAD. Не-git cwd, пустой репозиторий или
/// занятое имя — обычные ошибки (уходят модели как error ToolResult).
pub fn create_worktree(parent_cwd: &Path, name: &str) -> Result<WorkspaceInfo> {
    validate_name(name)?;
    let repo_root = PathBuf::from(
        run_git(parent_cwd, &["rev-parse", "--show-toplevel"])
            .context("subagent worktree requires a git repository")?,
    );
    let base_commit = run_git(&repo_root, &["rev-parse", "HEAD"])
        .context("subagent worktree requires at least one commit")?;

    let path = repo_root.join(WORKTREES_DIR).join(name);
    if path.exists() {
        bail!("worktree path already exists: {}", path.display());
    }
    let branch = format!("proteus/{name}");

    ensure_excluded(&repo_root)?;
    run_git(
        &repo_root,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            path.to_str()
                .with_context(|| format!("non-UTF-8 worktree path: {}", path.display()))?,
            "HEAD",
        ],
    )
    .context("failed to add git worktree")?;

    Ok(WorkspaceInfo::new(repo_root, path, branch, base_commit))
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum CleanupOutcome {
    Retained,
    Removed,
    // The checkout is absent, but cancellation or unique commits kept the branch.
    // This must not be published as a resumable checkout.
    BranchRetained,
}

/// Убирает неизменённый checkout и безопасно удаляет его ветку. Отмена
/// наблюдается до inspections и непосредственно перед каждой Git mutation;
/// уже запущенную команду проверка не останавливает и не откатывает.
/// Отсутствующий checkout с сохранённой веткой — отдельный исход, не resume.
pub(super) fn cleanup_worktree_if_unchanged(
    info: &WorkspaceInfo,
    mut is_cancelled: impl FnMut() -> bool,
) -> Result<CleanupOutcome> {
    if is_cancelled() {
        return Ok(if info.path.exists() {
            CleanupOutcome::Retained
        } else {
            CleanupOutcome::BranchRetained
        });
    }
    let branch_tip = run_git(
        &info.repo_root,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{}", info.branch),
        ],
    )
    .with_context(|| format!("failed to resolve saved worktree branch {}", info.branch))?;
    if !info.path.exists() {
        if is_cancelled() {
            return Ok(CleanupOutcome::BranchRetained);
        }
        run_git(&info.repo_root, &["worktree", "prune"])
            .context("failed to prune missing worktree bookkeeping")?;
        if branch_tip != info.base_commit || is_cancelled() {
            return Ok(CleanupOutcome::BranchRetained);
        }
        run_git(&info.repo_root, &["branch", "-d", &info.branch])
            .context("failed to safely delete missing worktree branch; branch preserved")?;
        return Ok(CleanupOutcome::Removed);
    }
    if branch_tip != info.base_commit {
        return Ok(CleanupOutcome::Retained);
    }

    let status = run_git(&info.path, &["status", "--porcelain"])
        .context("failed to check worktree status")?;
    if !status.is_empty() {
        return Ok(CleanupOutcome::Retained);
    }
    let head =
        run_git(&info.path, &["rev-parse", "HEAD"]).context("failed to resolve worktree HEAD")?;
    if head != info.base_commit {
        return Ok(CleanupOutcome::Retained);
    }

    let path = info
        .path
        .to_str()
        .with_context(|| format!("non-UTF-8 worktree path: {}", info.path.display()))?;
    if is_cancelled() {
        return Ok(CleanupOutcome::Retained);
    }
    run_git(&info.repo_root, &["worktree", "remove", path])
        .context("failed to remove git worktree")?;
    if is_cancelled() {
        return Ok(CleanupOutcome::BranchRetained);
    }
    run_git(&info.repo_root, &["branch", "-d", &info.branch])
        .context("failed to safely delete worktree branch; branch preserved")?;
    Ok(CleanupOutcome::Removed)
}

/// Имя идёт в путь и в имя ветки — только безопасный алфавит.
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("workspace name must not be empty");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        || name.starts_with('.')
    {
        bail!("workspace name must be [A-Za-z0-9._-]+ and not start with a dot: {name}");
    }
    Ok(())
}

/// Дописывает `/.proteus/` в `.git/info/exclude` основного checkout, чтобы
/// каталог worktree-ов не светился в `git status` (не трогая `.gitignore`).
fn ensure_excluded(repo_root: &Path) -> Result<()> {
    let git_dir = PathBuf::from(run_git(repo_root, &["rev-parse", "--git-common-dir"])?);
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        repo_root.join(git_dir)
    };
    let info_dir = git_dir.join("info");
    let exclude = info_dir.join("exclude");
    let existing = fs::read_to_string(&exclude).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == EXCLUDE_LINE) {
        return Ok(());
    }
    fs::create_dir_all(&info_dir)
        .with_context(|| format!("failed to create {}", info_dir.display()))?;
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(EXCLUDE_LINE);
    updated.push('\n');
    fs::write(&exclude, updated).with_context(|| format!("failed to write {}", exclude.display()))
}

/// Запускает git и возвращает trimmed stdout; не-нулевой exit code —
/// ошибка с stderr в тексте.
fn run_git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        run_git(dir, args).expect("git command");
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "test@test"]);
        git(dir.path(), &["config", "user.name", "test"]);
        fs::write(dir.path().join("file.txt"), "base\n").expect("seed file");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "base"]);
        dir
    }

    #[test]
    fn create_and_cleanup_unchanged_worktree() {
        let repo = init_repo();
        let info = create_worktree(repo.path(), "task-1").expect("worktree");

        assert!(info.path.is_dir());
        assert_eq!(info.branch, "proteus/task-1");
        assert_eq!(
            run_git(&info.path, &["rev-parse", "HEAD"]).unwrap(),
            info.base_commit
        );
        // Основной checkout не замусорен: .proteus/ исключён.
        assert_eq!(
            run_git(repo.path(), &["status", "--porcelain"]).unwrap(),
            ""
        );

        assert_eq!(
            cleanup_worktree_if_unchanged(&info, || false).expect("cleanup"),
            CleanupOutcome::Removed
        );
        assert!(!info.path.exists());
        assert!(
            run_git(repo.path(), &["branch", "--list", &info.branch])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cleanup_keeps_worktree_with_uncommitted_changes() {
        let repo = init_repo();
        let info = create_worktree(repo.path(), "task-2").expect("worktree");
        fs::write(info.path.join("file.txt"), "changed\n").expect("edit");

        assert_eq!(
            cleanup_worktree_if_unchanged(&info, || false).expect("cleanup"),
            CleanupOutcome::Retained
        );
        assert!(info.path.is_dir());
    }

    #[test]
    fn cleanup_keeps_worktree_with_commits() {
        let repo = init_repo();
        let info = create_worktree(repo.path(), "task-3").expect("worktree");
        fs::write(info.path.join("new.txt"), "work\n").expect("edit");
        git(&info.path, &["add", "."]);
        git(&info.path, &["commit", "-q", "-m", "child work"]);

        assert_eq!(
            cleanup_worktree_if_unchanged(&info, || false).expect("cleanup"),
            CleanupOutcome::Retained
        );
        assert!(info.path.is_dir());
    }

    #[test]
    fn cleanup_of_manually_removed_worktree_prunes_bookkeeping() {
        let repo = init_repo();
        let info = create_worktree(repo.path(), "task-4").expect("worktree");
        fs::remove_dir_all(&info.path).expect("manual removal");

        assert_eq!(
            cleanup_worktree_if_unchanged(&info, || false).expect("cleanup"),
            CleanupOutcome::Removed
        );
        assert!(
            run_git(repo.path(), &["branch", "--list", &info.branch])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cancellation_before_inspection_or_remove_preserves_clean_checkout() {
        for cancel_at in [1, 2] {
            let repo = init_repo();
            let info = create_worktree(repo.path(), "cancel-clean").unwrap();
            let cancellation = crate::contracts::CancellationToken::new();
            let mut observations = 0;
            let outcome = cleanup_worktree_if_unchanged(&info, || {
                observations += 1;
                // The second observation is after status/HEAD inspections,
                // immediately before removal. No scheduler timing is needed.
                if observations == cancel_at {
                    cancellation.cancel();
                }
                cancellation.is_cancelled()
            })
            .unwrap();
            assert_eq!(observations, cancel_at);
            assert_eq!(outcome, CleanupOutcome::Retained);
            assert!(info.path.is_dir());
            assert_eq!(
                run_git(repo.path(), &["rev-parse", &info.branch]).unwrap(),
                info.base_commit
            );
        }
    }

    #[test]
    fn cancellation_before_prune_preserves_missing_checkout_bookkeeping() {
        let repo = init_repo();
        let info = create_worktree(repo.path(), "cancel-prune").unwrap();
        fs::remove_dir_all(&info.path).unwrap();
        let before = run_git(repo.path(), &["worktree", "list", "--porcelain"]).unwrap();
        let cancellation = crate::contracts::CancellationToken::new();
        let mut observations = 0;
        let outcome = cleanup_worktree_if_unchanged(&info, || {
            observations += 1;
            if observations == 2 {
                cancellation.cancel();
            }
            cancellation.is_cancelled()
        })
        .unwrap();
        assert_eq!(observations, 2);
        assert_eq!(outcome, CleanupOutcome::BranchRetained);
        assert_eq!(
            run_git(repo.path(), &["worktree", "list", "--porcelain"]).unwrap(),
            before
        );
        assert_eq!(
            run_git(repo.path(), &["rev-parse", &info.branch]).unwrap(),
            info.base_commit
        );
    }

    #[test]
    fn cancellation_after_remove_or_prune_keeps_branch_without_claiming_checkout() {
        for checkout_missing in [false, true] {
            let repo = init_repo();
            let info = create_worktree(repo.path(), "cancel-branch").unwrap();
            if checkout_missing {
                fs::remove_dir_all(&info.path).unwrap();
            }
            let cancellation = crate::contracts::CancellationToken::new();
            let mut observations = 0;
            let outcome = cleanup_worktree_if_unchanged(&info, || {
                observations += 1;
                if observations == 3 {
                    cancellation.cancel();
                }
                cancellation.is_cancelled()
            })
            .unwrap();
            assert_eq!(observations, 3);
            assert_eq!(outcome, CleanupOutcome::BranchRetained);
            assert!(
                !info.path.exists(),
                "already dispatched removal is not undone"
            );
            assert!(
                !run_git(repo.path(), &["worktree", "list", "--porcelain"])
                    .unwrap()
                    .contains(info.path.to_str().unwrap())
            );
            assert_eq!(
                run_git(repo.path(), &["rev-parse", &info.branch]).unwrap(),
                info.base_commit
            );
        }
    }

    #[test]
    fn cleanup_preserves_unique_branch_commits_when_checkout_is_missing_or_detached() {
        for remove_checkout in [true, false] {
            let repo = init_repo();
            let info = create_worktree(repo.path(), "keep-result").unwrap();
            fs::write(info.path.join("result.txt"), "peer result\n").unwrap();
            git(&info.path, &["add", "."]);
            git(&info.path, &["commit", "-q", "-m", "unique peer result"]);
            let tip = run_git(&info.path, &["rev-parse", "HEAD"]).unwrap();
            if remove_checkout {
                git(
                    repo.path(),
                    &["worktree", "remove", info.path.to_str().unwrap()],
                );
            } else {
                git(&info.path, &["checkout", "--detach", &info.base_commit]);
            }
            assert_eq!(
                cleanup_worktree_if_unchanged(&info, || false).unwrap(),
                if remove_checkout {
                    CleanupOutcome::BranchRetained
                } else {
                    CleanupOutcome::Retained
                }
            );
            assert_eq!(
                run_git(repo.path(), &["rev-parse", &info.branch]).unwrap(),
                tip
            );
            assert_eq!(info.path.exists(), !remove_checkout);
        }
    }

    #[test]
    fn worktree_in_subdirectory_resolves_repo_root() {
        let repo = init_repo();
        let sub = repo.path().join("src");
        fs::create_dir(&sub).expect("subdir");
        let info = create_worktree(&sub, "task-5").expect("worktree");
        assert!(
            info.path
                .starts_with(repo.path().canonicalize().expect("canonical repo root"))
                || info.path.starts_with(repo.path())
        );
    }

    #[test]
    fn non_git_cwd_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = create_worktree(dir.path(), "task-6").unwrap_err();
        assert!(format!("{error:#}").contains("git repository"));
    }

    #[test]
    fn duplicate_and_invalid_names_are_rejected() {
        let repo = init_repo();
        create_worktree(repo.path(), "dup").expect("worktree");
        assert!(create_worktree(repo.path(), "dup").is_err());
        assert!(create_worktree(repo.path(), "").is_err());
        assert!(create_worktree(repo.path(), "../evil").is_err());
        assert!(create_worktree(repo.path(), ".hidden").is_err());
    }
}

use std::{fs, path::Path};

use anyhow::{Result, ensure};
use serde::Serialize;

use super::{
    super::MAX_BYTES,
    command::{Output, run},
    repository, revision, validate,
};

#[derive(Serialize)]
pub(crate) struct Diff {
    path: String,
    kind: &'static str,
    patch: Option<String>,
    revision: Option<String>,
}

impl Diff {
    pub(super) fn unavailable(path: String) -> Self {
        Self {
            path,
            kind: "unavailable",
            patch: None,
            revision: None,
        }
    }

    /// Classifies the output of a single-file `git diff` as text, binary or too large.
    pub(super) fn classify(mut self, output: Output) -> Result<Self> {
        if output.truncated {
            self.kind = "too_large";
            return Ok(self);
        }
        ensure!(output.success || output.different, "cannot read Git diff");
        self.kind = "binary";
        if let Ok(patch) = String::from_utf8(output.bytes)
            && !patch.contains('\0')
            && !patch
                .lines()
                .any(|line| line.starts_with("Binary files ") && line.ends_with(" differ"))
        {
            self.kind = "text";
            self.patch = Some(patch);
        }
        Ok(self)
    }
}

pub(crate) fn diff(root: &Path, relative: String) -> Result<Diff> {
    let path = validate(root, &relative)?;
    if !repository(root)? {
        return Ok(Diff::unavailable(relative));
    }
    let (head, index) = source_state(root, &relative)?;
    let before = source_revision(root, &relative, &head, &index);
    let mut result = working_diff(root, path, relative, head.success, !index.bytes.is_empty())?;
    let (head, index) = source_state(root, &result.path)?;
    let after = source_revision(root, &result.path, &head, &index);
    // Never associate a patch with a newer source snapshot if a write,
    // checkout or staging operation raced its construction.
    result.revision = before.filter(|before| Some(before) == after.as_ref());
    Ok(result)
}

fn source_state(root: &Path, relative: &str) -> Result<(Output, Output)> {
    let head = run(root, &["rev-parse", "--verify", "--quiet", "HEAD"])?;
    let index = run(root, &["ls-files", "--stage", "-z", "--", relative])?.checked("ls-files")?;
    Ok((head, index))
}

fn source_revision(root: &Path, path: &str, head: &Output, index: &Output) -> Option<String> {
    let oid = std::str::from_utf8(&head.bytes).ok()?.trim();
    let stages = revision::stages(&index.bytes)?;
    revision::token(root, path, head.success.then_some(oid), &stages)
}

fn working_diff(
    root: &Path,
    path: std::path::PathBuf,
    relative: String,
    head: bool,
    tracked: bool,
) -> Result<Diff> {
    let mut result = Diff::unavailable(relative);
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Ok(result);
            }
            if metadata.len() > MAX_BYTES as u64 {
                result.kind = "too_large";
                return Ok(result);
            }
            Some(metadata)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let in_head = if head {
        !run(
            root,
            &["ls-tree", "--name-only", "HEAD", "--", &result.path],
        )?
        .bytes
        .is_empty()
    } else {
        false
    };
    let output = if head && (tracked || in_head) {
        run(
            root,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--no-renames",
                "--relative",
                "HEAD",
                "--",
                &result.path,
            ],
        )?
    } else if metadata.is_some() {
        #[cfg(unix)]
        let null = "/dev/null";
        #[cfg(windows)]
        let null = "NUL";
        run(
            root,
            &[
                "diff",
                "--no-index",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--",
                null,
                &format!("./{}", result.path),
            ],
        )?
    } else {
        return Ok(result);
    };
    result.classify(output)
}

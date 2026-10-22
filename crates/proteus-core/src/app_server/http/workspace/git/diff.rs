use std::{fs, path::Path};

use anyhow::{Result, ensure};
use serde::Serialize;

use super::{
    super::MAX_BYTES,
    command::{Output, run},
    repository, validate,
};

#[derive(Serialize)]
pub(crate) struct Diff {
    path: String,
    kind: &'static str,
    patch: Option<String>,
}

impl Diff {
    pub(super) fn unavailable(path: String) -> Self {
        Self {
            path,
            kind: "unavailable",
            patch: None,
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
    let mut result = Diff::unavailable(relative);
    if !repository(root)? {
        return Ok(result);
    }
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
    let head = run(root, &["rev-parse", "--verify", "HEAD"])?;
    let tracked = run(root, &["ls-files", "--error-unmatch", "--", &result.path])?.success;
    let in_head = if head.success {
        !run(
            root,
            &["ls-tree", "--name-only", "HEAD", "--", &result.path],
        )?
        .bytes
        .is_empty()
    } else {
        false
    };
    let output = if head.success && (tracked || in_head) {
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

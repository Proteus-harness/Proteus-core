use std::{fs, path::Path};

use anyhow::{Result, ensure};
use serde::Serialize;

use super::{super::MAX_BYTES, command::run, repository, validate};

#[derive(Serialize)]
pub(crate) struct Diff {
    path: String,
    kind: &'static str,
    patch: Option<String>,
}

pub(crate) fn diff(root: &Path, relative: String) -> Result<Diff> {
    let path = validate(root, &relative)?;
    let mut result = Diff {
        path: relative,
        kind: "unavailable",
        patch: None,
    };
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
    if output.truncated {
        result.kind = "too_large";
        return Ok(result);
    }
    ensure!(output.success || output.different, "cannot read Git diff");
    result.kind = "binary";
    if let Ok(patch) = String::from_utf8(output.bytes)
        && !patch.contains('\0')
        && !patch
            .lines()
            .any(|line| line.starts_with("Binary files ") && line.ends_with(" differ"))
    {
        result.kind = "text";
        result.patch = Some(patch);
    }
    Ok(result)
}

//! Files touched by an `apply_patch` envelope and their diff lines.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchOperation {
    Add,
    Delete,
    Update,
    Move,
}

impl PatchOperation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Add => "создан",
            Self::Delete => "удалён",
            Self::Update => "изменён",
            Self::Move => "перемещён",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLine {
    Added,
    Removed,
    Hunk,
    Context,
}

impl DiffLine {
    pub fn of(line: &str) -> Self {
        if line.starts_with("@@") {
            Self::Hunk
        } else if line.starts_with('+') {
            Self::Added
        } else if line.starts_with('-') {
            Self::Removed
        } else {
            Self::Context
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchFile {
    pub path: String,
    pub operation: PatchOperation,
    pub additions: usize,
    pub deletions: usize,
    /// Hunk lines without the file header.
    pub body: String,
}

pub fn patch_files(patch: &str) -> Vec<PatchFile> {
    let mut files: Vec<PatchFile> = Vec::new();
    for line in patch.lines() {
        if line == "*** Begin Patch" || line == "*** End Patch" {
            continue;
        }
        if let Some((operation, path)) = file_header(line) {
            files.push(PatchFile {
                path: path.to_owned(),
                operation,
                additions: 0,
                deletions: 0,
                body: String::new(),
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if let Some(target) = line.strip_prefix("*** Move to: ") {
            file.operation = PatchOperation::Move;
            file.path = format!("{} → {target}", file.path);
            continue;
        }
        match DiffLine::of(line) {
            DiffLine::Added => file.additions += 1,
            DiffLine::Removed => file.deletions += 1,
            DiffLine::Hunk | DiffLine::Context => {}
        }
        if !file.body.is_empty() {
            file.body.push('\n');
        }
        file.body.push_str(line);
    }
    files
}

pub fn patch_stats(files: &[PatchFile]) -> (usize, usize) {
    files.iter().fold((0, 0), |(added, removed), file| {
        (added + file.additions, removed + file.deletions)
    })
}

fn file_header(line: &str) -> Option<(PatchOperation, &str)> {
    [
        ("*** Add File: ", PatchOperation::Add),
        ("*** Delete File: ", PatchOperation::Delete),
        ("*** Update File: ", PatchOperation::Update),
    ]
    .into_iter()
    .find_map(|(prefix, operation)| line.strip_prefix(prefix).map(|path| (operation, path)))
}

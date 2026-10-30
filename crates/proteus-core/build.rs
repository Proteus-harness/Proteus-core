use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    let reference = git(&["symbolic-ref", "-q", "HEAD"]);
    for name in [Some("HEAD"), reference.as_deref()].into_iter().flatten() {
        if let Some(path) = git(&["rev-parse", "--git-path", name]) {
            let path = Path::new(&path);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            } else if name != "HEAD" {
                // Packed branches have no loose ref: watching the missing file
                // makes every build dirty. Watch its nearest existing directory
                // within refs to detect a later commit creating the loose ref.
                if let Some(refs) = git(&["rev-parse", "--git-path", "refs"]) {
                    if let Some(parent) = path
                        .ancestors()
                        .skip(1)
                        .take_while(|parent| parent.starts_with(Path::new(&refs)))
                        .find(|parent| parent.is_dir())
                    {
                        println!("cargo:rerun-if-changed={}", parent.display());
                    }
                }
                if let Some(packed) = git(&["rev-parse", "--git-path", "packed-refs"]) {
                    if Path::new(&packed).is_file() {
                        println!("cargo:rerun-if-changed={packed}");
                    }
                }
            }
        }
    }
    // Reftable keeps shared refs in the common directory and a linked
    // worktree's HEAD in its own Git directory. Watch only existing stores.
    for option in ["--git-common-dir", "--git-dir"] {
        if let Some(directory) = git(&["rev-parse", option]) {
            let store = Path::new(&directory).join("reftable");
            if store.is_dir() {
                println!("cargo:rerun-if-changed={}", store.display());
            }
        }
    }
    let revision =
        git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=PROTEUS_BUILD_COMMIT={revision}");
}

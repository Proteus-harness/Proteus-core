use super::*;
use crate::core::AppConfig;

async fn loaded(path: &Path) -> ConfigInputs {
    let mut inputs = ConfigInputs::watching();
    AppConfig::load_observed(Some(path), &mut inputs)
        .await
        .unwrap();
    assert!(inputs.is_current());
    inputs
}

#[tokio::test]
async fn watches_directory_members_external_includes_and_instruction_files() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("profile");
    std::fs::create_dir(&dir).unwrap();
    let fragment = root.path().join("fragment.toml");
    let instructions = dir.join("rules.md");
    std::fs::write(&fragment, "").unwrap();
    std::fs::write(&instructions, "first").unwrap();
    std::fs::write(
        dir.join("base.toml"),
        "include='../fragment.toml'\n[[instructions]]\nkind='System'\npriority=100\nfile='rules.md'\n",
    )
    .unwrap();
    let inputs = loaded(&dir).await;
    std::fs::write(&fragment, "# edit outside the profile directory").unwrap();
    assert!(!inputs.is_current());
    let inputs = loaded(&dir).await;
    let before = std::fs::metadata(&instructions).unwrap();
    std::fs::write(&instructions, "other").unwrap();
    // Establish a real file-metadata signal rather than relying on clock granularity.
    let modified = before
        .modified()
        .unwrap()
        .checked_add(std::time::Duration::from_secs(2))
        .unwrap();
    std::fs::File::options()
        .write(true)
        .open(&instructions)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let after = std::fs::metadata(&instructions).unwrap();
    assert_eq!(before.len(), after.len(), "retain same-length coverage");
    assert_ne!(
        before.modified().unwrap(),
        after.modified().unwrap(),
        "fixture requires a read-back metadata change"
    );
    assert!(!inputs.is_current());
    let inputs = loaded(&dir).await;
    std::fs::write(dir.join("new.toml"), "").unwrap();
    // Directory size and timestamps need not change for every addition.
    // Inject an explicit directory-metadata prerequisite into the SAME comparison
    // routine; real same-stamp membership discovery is covered by profile_watch.
    let directory = std::fs::canonicalize(&dir).unwrap();
    let recorded = inputs.files.as_ref().unwrap();
    let original = recorded
        .get(&directory)
        .expect("directory dependency must be observed");
    let mut changed = original.clone();
    match &mut changed {
        Stamp::Present { modified, .. } => {
            *modified = Some(
                modified
                    .unwrap_or(SystemTime::UNIX_EPOCH)
                    .checked_add(std::time::Duration::from_secs(2))
                    .unwrap(),
            );
        }
        _ => panic!("fixture requires a present directory"),
    }
    assert_ne!(&changed, original, "injected directory stamp must differ");
    assert!(inputs.is_current_with(|path| recorded.get(path).unwrap().clone()));
    assert!(!inputs.is_current_with(|path| {
        if path == directory.as_path() {
            changed.clone()
        } else {
            recorded.get(path).unwrap().clone()
        }
    }));
    let inputs = loaded(&dir).await;
    std::fs::remove_file(dir.join("new.toml")).unwrap();
    assert!(!inputs.is_current());
}

#[tokio::test]
async fn failed_load_tracks_missing_include_for_recovery() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.toml");
    let fragment = root.path().join("missing.toml");
    std::fs::write(&path, "include='missing.toml'\n").unwrap();
    let mut inputs = ConfigInputs::watching();
    assert!(
        AppConfig::load_observed(Some(&path), &mut inputs)
            .await
            .is_err()
    );
    assert!(inputs.is_current());
    std::fs::write(&fragment, "").unwrap();
    assert!(!inputs.is_current());
    loaded(&path).await;
}

#[test]
fn tracks_new_plugin_metadata_and_does_not_hide_writes_during_a_load() {
    let root = tempfile::tempdir().unwrap();
    let mut config = crate::domain::AddonConfig::default();
    config
        .plugins
        .push(serde_json::from_value(serde_json::json!({"path":"plugin"})).unwrap());
    let plugin = root.path().join("plugin");
    std::fs::create_dir(&plugin).unwrap();
    let mut inputs = ConfigInputs::watching();
    inputs.observe_plugins(&config, root.path());
    assert!(inputs.is_current());
    std::fs::write(plugin.join("plugin.json"), "{}").unwrap();
    inputs.observe_plugins(&config, root.path());
    assert!(
        !inputs.is_current(),
        "later observations must not mask concurrent writes"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn detects_atomic_replacement_and_retargeted_include_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.toml");
    let target = root.path().join("target.toml");
    let link = root.path().join("link.toml");
    std::fs::write(&path, "include='link.toml'\n").unwrap();
    std::fs::write(&target, "").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let inputs = loaded(&path).await;
    let replacement = root.path().join("replacement.toml");
    std::fs::write(&replacement, "").unwrap();
    let modified = std::fs::metadata(&target).unwrap().modified().unwrap();
    std::fs::File::options()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    std::fs::rename(&replacement, &target).unwrap();
    assert!(
        !inputs.is_current(),
        "same-size/mtime atomic saves still change identity"
    );
    let inputs = loaded(&path).await;
    std::fs::write(&replacement, "").unwrap();
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&replacement, &link).unwrap();
    assert!(!inputs.is_current());
}

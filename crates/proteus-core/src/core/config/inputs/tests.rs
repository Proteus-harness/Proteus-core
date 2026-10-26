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
    std::fs::write(&instructions, "other").unwrap();
    assert!(!inputs.is_current());
    let inputs = loaded(&dir).await;
    std::fs::write(dir.join("new.toml"), "").unwrap();
    assert!(!inputs.is_current());
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

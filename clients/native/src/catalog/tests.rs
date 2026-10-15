use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use super::{schema::Kind, *};
use crate::{packages::Contents, settings::Draft};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from("/tmp/opencode")
            .join(format!("proteus-catalog-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn locations(&self) -> Locations {
        Locations {
            cache: self.0.join("cache"),
            extensions: self.0.join("extensions"),
            plugins: self.0.join("plugins"),
            skills: self.0.join("skills"),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/catalog")
}
fn source(path: &Path) -> Source {
    Source {
        url: path.to_str().unwrap().to_string(),
        branch: None,
    }
}
fn git_at(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn commit(path: &Path) {
    git_at(path, &["add", "."]);
    git_at(
        path,
        &[
            "-c",
            "user.name=Catalog Test",
            "-c",
            "user.email=catalog@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "fixture",
        ],
    );
}

#[test]
fn git_refresh_keeps_last_good_checkout_and_reports_bad_entries() {
    let fixture = Fixture::new();
    let repo = Contents::folder(&example())
        .unwrap()
        .install(&fixture.0, "repo", false)
        .unwrap();
    git_at(&repo, &["init", "-q", "-b", "main"]);
    commit(&repo);
    let source = source(&repo);
    let cache = fixture.locations().cache;
    let first = git::refresh(&cache, &source).unwrap();
    assert_eq!(first.entries.len(), 4);
    let mut index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(repo.join(schema::INDEX)).unwrap()).unwrap();
    index["entries"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"id":"broken","kind":"unknown"}));
    index["entries"][0]["version"] = "2.0.0".into();
    fs::write(repo.join(schema::INDEX), index.to_string()).unwrap();
    // Removed files must disappear on refresh as well, not survive in git's index.
    fs::remove_file(repo.join("client/demo/README.md")).unwrap();
    commit(&repo);
    let second = git::refresh(&cache, &source).unwrap();
    assert_eq!(second.entries[0].version, "2.0.0");
    assert_eq!(second.problems.len(), 1);
    assert!(
        schema::readme(&second, &second.entries[0])
            .unwrap()
            .is_none()
    );
    fs::write(repo.join(schema::INDEX), "{bad").unwrap();
    commit(&repo);
    assert!(git::refresh(&cache, &source).is_err());
    assert_eq!(
        git::cached(&cache, &source).unwrap().entries[0].version,
        "2.0.0"
    );
    let missing = Source {
        branch: Some("missing".into()),
        ..source.clone()
    };
    assert!(git::refresh(&cache, &missing).is_err());
}

#[test]
fn installation_binding_update_and_removal_use_the_existing_formats() {
    let fixture = Fixture::new();
    let locations = fixture.locations();
    let catalog = schema::read(&example(), "example").unwrap();
    let source = source(&example());
    let profile = fixture.0.join("profile.toml");
    let mut draft = Draft::default();
    for entry in &catalog.entries {
        let installed =
            install::prepare(&locations, &source, &catalog, entry, None, &profile, &[]).unwrap();
        install::connect(&mut draft, &installed, None).unwrap();
        assert!(install::connected(&installed, &draft));
        let mut newer = entry.clone();
        newer.version = "2.0.0".into();
        let updated = install::prepare(
            &locations,
            &source,
            &catalog,
            &newer,
            Some(&installed),
            &profile,
            &[],
        )
        .unwrap();
        install::connect(&mut draft, &updated, Some(&installed)).unwrap();
        if entry.kind == Kind::AgentPlugin {
            let Target::Folder { path: old } = &installed.target else {
                panic!()
            };
            let Target::Folder { path: new } = &updated.target else {
                panic!()
            };
            assert_ne!(old, new);
            assert!(
                old.join("plugin.json").is_file(),
                "saved versions stay intact"
            );
            assert_eq!(draft.addons.addons.plugins.len(), 1);
            // A discarded update leaves its files, not a durable receipt.
            // Retrying against the saved receipt must not collide with them.
            let retried = install::prepare(
                &locations,
                &source,
                &catalog,
                &newer,
                Some(&installed),
                &profile,
                &[],
            )
            .unwrap();
            assert_ne!(retried.target, updated.target);
            assert!(old.join("plugin.json").is_file());
        }
        install::disconnect(&mut draft, &updated).unwrap();
        install::remove_files(&locations, &updated).unwrap();
        if entry.kind == Kind::AgentPlugin {
            let Target::Folder { path } = &updated.target else {
                panic!()
            };
            assert!(path.is_dir());
        } else {
            assert!(!install::connected(&updated, &draft));
        }
    }
    let extension = &catalog.entries[0];
    assert!(
        install::prepare(
            &locations,
            &source,
            &catalog,
            extension,
            None,
            &profile,
            &[extension.id.clone()]
        )
        .is_err()
    );
}

#[test]
fn mcp_conflicts_dont_overwrite_manual_edits_and_receipts_are_profile_scoped() {
    let fixture = Fixture::new();
    let catalog = schema::read(&example(), "example").unwrap();
    let entry = catalog
        .entries
        .iter()
        .find(|entry| entry.kind == Kind::McpServer)
        .unwrap();
    let source = source(&example());
    let profile = fixture.0.join("a.toml");
    let installed = install::prepare(
        &fixture.locations(),
        &source,
        &catalog,
        entry,
        None,
        &profile,
        &[],
    )
    .unwrap();
    let mut prefs = Preferences::default();
    prefs.remember(installed.clone());
    assert!(install::receipt(&prefs.installed, &source, entry, &profile).is_some());
    assert!(
        install::receipt(&prefs.installed, &source, entry, &fixture.0.join("b.toml")).is_none()
    );
    let mut draft = Draft::default();
    install::connect(&mut draft, &installed, None).unwrap();
    let baseline = draft.clone();
    assert!(install::connect(&mut draft, &installed, None).is_err());
    assert_eq!(draft, baseline);
    draft.addons.mcp_servers[0].command = "edited".into();
    let edited = draft.clone();
    assert!(install::connect(&mut draft, &installed, Some(&installed)).is_err());
    assert!(install::disconnect(&mut draft, &installed).is_err());
    assert_eq!(draft, edited);
    // Receipts persist through discard: reconnecting after discard remains possible.
    draft = Draft::default();
    install::connect(&mut draft, &installed, Some(&installed)).unwrap();
    assert_eq!(draft, baseline);
}

#[test]
fn plugin_zip_and_invalid_catalog_paths_share_checked_package_io() {
    use std::io::Write;
    let fixture = Fixture::new();
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file("plugin.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer
        .write_all(&fs::read(example().join("plugins/review/plugin.json")).unwrap())
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let archive = fixture.0.join("plugin.zip");
    fs::write(&archive, bytes).unwrap();
    let path = install::import_zip(&fixture.locations(), &archive).unwrap();
    assert!(path.join("plugin.json").is_file());
    assert!(schema::relative(Path::new("../outside")).is_err());
    assert!(schema::relative(Path::new("/absolute")).is_err());
    assert!(schema::relative(Path::new("a/.git/config")).is_err());
    let mut catalog = schema::read(&example(), "example").unwrap();
    catalog.entries[0].path = "../outside".into();
    assert!(schema::directory(&catalog.root, &catalog.entries[0]).is_err());
    let mut broken = Source {
        url: "-x".into(),
        branch: None,
    };
    assert!(git::validate(&broken).is_err());
    broken.url = "x".into();
    broken.branch = Some("--bad".into());
    assert!(git::validate(&broken).is_err());
}

#[test]
fn repository_without_index_lists_agent_skills_as_installable_entries() {
    let fixture = Fixture::new();
    let root = fixture.0.join("repo");
    let skill = |dir: &str, front: &str| {
        let path = root.join(dir);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("SKILL.md"),
            format!("---\n{front}\n---\n\n# Body\n"),
        )
        .unwrap();
    };
    skill("skills/.curated/pdf", "name: pdf\ndescription: Read PDFs");
    // Nested folders belong to the enclosing skill.
    skill(
        "skills/.curated/pdf/forms",
        "name: forms\ndescription: Part of pdf",
    );
    skill("skills/.curated/docs", "name: docs\ndescription: Curated");
    skill("skills/.system/docs", "name: docs\ndescription: System");
    skill("template", "name: template-skill\ndescription: Mismatch");
    skill("tools/lint", "name: lint\ndescription: >\n  Folded\n  text");
    let catalog = schema::read(&root, "owner/skills").unwrap();
    assert_eq!(catalog.title, "owner/skills");
    let ids = catalog
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["pdf", "lint"]);
    assert_eq!(catalog.problems.len(), 2, "{:?}", catalog.problems);
    let categories = catalog
        .categories
        .iter()
        .map(|category| category.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(categories, ["skills/.curated", "tools"]);
    let (pdf, lint) = (&catalog.entries[0], &catalog.entries[1]);
    assert_eq!(
        (pdf.kind, lint.description.as_str()),
        (Kind::Skill, "Folded text")
    );
    assert_eq!(
        schema::readme(&catalog, pdf).unwrap().as_deref(),
        Some("# Body")
    );

    let locations = fixture.locations();
    let installed = install::prepare(
        &locations,
        &source(&root),
        &catalog,
        pdf,
        None,
        &fixture.0,
        &[],
    )
    .unwrap();
    assert!(locations.skills.join("pdf/forms/SKILL.md").is_file());
    fs::write(root.join("skills/.curated/pdf/forms/extra.md"), "more").unwrap();
    let changed = schema::read(&root, "owner/skills").unwrap();
    assert_ne!(changed.entries[0].version, installed.version);
    fs::remove_dir_all(&root).unwrap();
    fs::create_dir_all(&root).unwrap();
    assert!(schema::read(&root, "owner/skills").is_err());

    for url in [
        "https://github.com/owner/skills.git/",
        "git@github.com:owner/skills",
    ] {
        let source = Source {
            url: url.into(),
            branch: None,
        };
        assert_eq!(source.name(), "owner/skills");
    }
}

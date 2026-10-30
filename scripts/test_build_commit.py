"""Build-commit invalidation using an offline std-only crate, not Core or GPUI."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
BUILD_SCRIPT = ROOT / "crates/proteus-core/build.rs"
FIXTURES = ROOT / "target/verification/build-commit"


class BuildCommitIntegrationTests(unittest.TestCase):
    def setUp(self):
        FIXTURES.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(prefix="proteus build commit ", dir=FIXTURES)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.checkout = self.root / "checkout"
        (self.checkout / "src").mkdir(parents=True)
        (self.checkout / "Cargo.toml").write_text(
            '[package]\nname = "build-commit-fixture"\nversion = "0.1.0"\n'
            'edition = "2024"\n[workspace]\n'
        )
        (self.checkout / "Cargo.lock").write_text(
            'version = 4\n[[package]]\nname = "build-commit-fixture"\nversion = "0.1.0"\n'
        )
        (self.checkout / "src/main.rs").write_text(
            'fn main() { println!("{}", env!("PROTEUS_BUILD_COMMIT")); }\n'
        )
        (self.checkout / ".gitignore").write_text("/target/\n")
        shutil.copyfile(BUILD_SCRIPT, self.checkout / "build.rs")
        # Git settings/hooks must not come from the developer's real repositories.
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith("GIT_")}
        self.env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                        GIT_CEILING_DIRECTORIES=str(self.root),
                        CARGO_TARGET_DIR=str(self.root / "target"))

    def git(self, *args):
        return subprocess.check_output(
            ["git", *args], cwd=self.checkout, env=self.env,
            text=True, stderr=subprocess.PIPE,
        ).strip()

    def initialize(self, branch="feature/nested/branch", ref_format=None):
        options = [f"--ref-format={ref_format}"] if ref_format else []
        self.git("init", "--template=", "--initial-branch", branch, *options)
        self.git("config", "user.name", "Build Fixture")
        self.git("config", "user.email", "build-fixture@example.invalid")
        self.git("config", "commit.gpgsign", "false")
        self.git("add", ".")
        self.git("commit", "-m", "fixture")

    def commit(self):
        self.git("commit", "--allow-empty", "-m", "advance fixture")
        return self.git("rev-parse", "--short=12", "HEAD")

    def build(self, *, fresh, revision=None):
        result = subprocess.run(
            ["cargo", "build", "--offline", "--locked", "--release",
             "--message-format=json-render-diagnostics"],
            cwd=self.checkout, env=self.env, check=True, capture_output=True, text=True,
        )
        records = [json.loads(line) for line in result.stdout.splitlines()
                   if line.startswith("{")]
        artifacts = [record for record in records
                     if record.get("reason") == "compiler-artifact"
                     and record["target"]["kind"] == ["bin"]
                     and record["target"]["name"] == "build-commit-fixture"]
        self.assertEqual(len(artifacts), 1, result.stdout)
        self.assertIs(artifacts[0]["fresh"], fresh, result.stderr)
        output = subprocess.check_output(
            [artifacts[0]["executable"]], cwd=self.checkout, env=self.env, text=True,
        ).strip()
        expected = revision if revision is not None else self.git("rev-parse", "--short=12", "HEAD")
        self.assertEqual(output, expected)
        return output

    def loose_ref(self):
        name = self.git("symbolic-ref", "-q", "HEAD")
        path = Path(self.git("rev-parse", "--git-path", name))
        return (self.checkout / path).resolve()

    def test_packed_nested_ref_stays_fresh_then_tracks_new_loose_ref(self):
        self.initialize()
        loose = self.loose_ref()
        self.git("pack-refs", "--all", "--prune")
        self.assertFalse(loose.exists())
        # Cover missing nested directories, not just a missing final ref file.
        refs = (self.checkout / self.git("rev-parse", "--git-path", "refs")).resolve()
        parent = loose.parent
        while parent != refs:
            if parent.exists():
                parent.rmdir()
            parent = parent.parent
        self.build(fresh=False)
        self.build(fresh=True)
        revision = self.commit()
        self.assertTrue(loose.is_file())
        self.build(fresh=False, revision=revision)
        self.build(fresh=True, revision=revision)

    def test_loose_to_packed_and_packed_update_refresh_revision(self):
        self.initialize()
        self.build(fresh=False)
        self.build(fresh=True)
        self.git("pack-refs", "--all", "--prune")
        self.build(fresh=False)
        self.build(fresh=True)
        revision = self.commit()
        # The new loose ref disappears before Cargo can observe it.
        self.git("pack-refs", "--all", "--prune")
        self.assertFalse(self.loose_ref().exists())
        self.build(fresh=False, revision=revision)
        self.build(fresh=True, revision=revision)

    def test_detached_head_tracks_commit_not_tags_or_unrelated_dirty_files(self):
        self.initialize()
        self.git("checkout", "--detach", "HEAD")
        revision = self.build(fresh=False)
        self.build(fresh=True)
        self.git("-c", "tag.gpgsign=false", "tag", "version-only")
        (self.checkout / "README.md").write_text("unrelated dirty documentation\n")
        self.build(fresh=True, revision=revision)
        revision = self.commit()
        self.build(fresh=False, revision=revision)
        self.build(fresh=True, revision=revision)

    def test_packed_linked_worktree_tracks_common_ref_and_own_head(self):
        self.initialize(branch="main")
        linked = self.root / "linked"
        self.git("worktree", "add", "-b", "worktree/nested/branch", str(linked), "HEAD")
        self.checkout = linked
        self.git("pack-refs", "--all", "--prune")
        self.assertFalse(self.loose_ref().exists())
        self.build(fresh=False)
        self.build(fresh=True)
        revision = self.commit()
        self.build(fresh=False, revision=revision)
        self.build(fresh=True, revision=revision)
        self.git("checkout", "--detach", "HEAD~1")
        self.build(fresh=False)
        self.build(fresh=True)

    def test_reftable_tracks_shared_refs_and_detached_worktree_head(self):
        help_text = subprocess.run(
            ["git", "init", "-h"], env=self.env, capture_output=True, text=True,
        )
        if "--ref-format" not in help_text.stdout + help_text.stderr:
            self.skipTest("installed Git has no reftable support")
        self.initialize(branch="main", ref_format="reftable")
        self.build(fresh=False)
        self.build(fresh=True)
        revision = self.commit()
        self.build(fresh=False, revision=revision)
        self.build(fresh=True, revision=revision)
        linked = self.root / "linked"
        self.git("worktree", "add", "--detach", str(linked), "HEAD")
        self.checkout = linked
        self.build(fresh=False)
        self.build(fresh=True)
        revision = self.commit()
        self.build(fresh=False, revision=revision)
        self.build(fresh=True, revision=revision)

    def test_source_without_git_keeps_unknown_revision(self):
        self.build(fresh=False, revision="unknown")
        self.build(fresh=True, revision="unknown")


if __name__ == "__main__":
    unittest.main()

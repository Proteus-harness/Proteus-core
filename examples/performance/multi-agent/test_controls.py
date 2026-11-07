"""Synthetic identity and evidence guards; never signal an existing process."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("legacy_proc", HERE / "proc.py")
proc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proc)

class ControlsTests(unittest.TestCase):
    def test_reused_root_and_its_children_are_never_adopted(self):
        original = {100: {"start_ticks":10,"parent":1}}
        replaced = {100: {"start_ticks":99,"parent":1}, 101:{"start_ticks":100,"parent":100}}
        with patch.object(proc, "table", side_effect=[original,replaced]):
            tree = proc.Tree(100)
            self.assertEqual(tree.sample()["processes"], [])
        self.assertEqual(tree.owned, {})

    def test_refused_run_preserves_existing_binary_manifest(self):
        with tempfile.TemporaryDirectory(dir=HERE) as temporary:
            base = Path(temporary)
            executable = base / "synthetic"
            executable.write_text("not executed")
            output = base / "evidence"
            output.mkdir()
            manifest = output / "binaries.json"
            manifest.write_bytes(b"original evidence")
            run = subprocess.run([sys.executable, str(HERE / "run.py"),
                "--driver", str(executable), "--binary", str(executable), "--module", str(executable),
                "--output", str(output)], capture_output=True, text=True)
            self.assertNotEqual(run.returncode, 0)
            self.assertIn("refusing to overwrite evidence", run.stderr)
            self.assertEqual(manifest.read_bytes(), b"original evidence")

if __name__ == "__main__": unittest.main()

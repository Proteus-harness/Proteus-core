"""Private clone inventory/probe controls; no Core runtime, model or user data."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from clone_read import PrivateTree, canonical_inventory, clone_source, validate_memory_trace


class CloneInventoryTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()
        self.workspace = self.root / "workspace"
        self.workspace.mkdir()
        self.source = self.root / "sessions" / "|".join(self.workspace.parts[1:]) / "0000000001"
        (self.source / "blobs").mkdir(parents=True)
        (self.source / "session.json").write_text(json.dumps({
            "schema_version": 4, "journal_schema_version": 21,
            "session_id": "00000000-0000-0000-0000-000000000001",
            "workspace_path": str(self.workspace)}))
        # Inventory tests need bytes, not a fabricated executable journal.
        (self.source / "journal.jsonl").write_bytes(b"synthetic canonical bytes\n")
        payload = b'{"synthetic":true}'
        self.blob = self.source / "blobs" / (hashlib.sha256(payload).hexdigest() + ".json")
        self.blob.write_bytes(payload)
        self.temporary = self.source / "blobs" / (
            "." + "a" * 64 + ".tmp.00000000-0000-0000-0000-000000000002")
        self.spec = self.root / "private-source.json"
        self.spec.write_text(json.dumps({"source_session_dir": str(self.source),
                                         "workspace_path": str(self.workspace)}))
        self.spec.chmod(0o600)
        self.output = self.root / "output"
        self.output.mkdir(mode=0o700)
        self.args = SimpleNamespace(source_spec=self.spec, output=self.output,
                                    module=Path(sys.executable).resolve(), deadline=time.monotonic() + 10)

    def test_exact_writer_temporary_is_hashed_privately_and_not_copied(self):
        self.temporary.write_bytes(b"unfinished synthetic blob")
        canonical, temporary = canonical_inventory(self.source)
        self.assertEqual(len(canonical), 3)
        self.assertEqual(len(temporary), 1)
        self.assertEqual(next(iter(temporary.values()))["bytes"], self.temporary.stat().st_size)
        private = {}
        _, copied, _, _, before, _ = clone_source(self.args, private)
        self.assertEqual(canonical_inventory(copied), (before, {}))
        self.assertFalse((copied / "blobs" / self.temporary.name).exists())
        self.assertEqual(private["temporary_before"], temporary)
        self.assertEqual(self.temporary.read_bytes(), b"unfinished synthetic blob")

    def test_concurrent_canonical_or_temporary_mutation_is_detected(self):
        self.temporary.write_bytes(b"original temporary")
        original_run = subprocess.run
        for kind in ("canonical", "temporary"):
            with self.subTest(kind=kind):
                self.args.output = self.root / ("output-" + kind)
                self.args.output.mkdir()
                target = self.source / "journal.jsonl" if kind == "canonical" else self.temporary
                changed = False

                def copy_then_mutate(*args, **kwargs):
                    nonlocal changed
                    result = original_run(*args, **kwargs)
                    if not changed:
                        target.write_bytes(target.read_bytes() + b"changed")
                        changed = True
                    return result

                with patch("clone_read.subprocess.run", side_effect=copy_then_mutate):
                    with self.assertRaisesRegex(ValueError, "hash mismatch|concurrent source change"):
                        clone_source(self.args, {})

    def test_unrecognized_files_and_symlinks_stay_rejected(self):
        cases = ("unrecognized.tmp", "." + "a" * 64 + ".tmp.not-a-record-uuid",
                 "a" * 63 + ".json", "symlink")
        for name in cases:
            with self.subTest(case=name):
                path = self.source / "blobs" / name
                if name == "symlink":
                    path.symlink_to(self.blob)
                else:
                    path.write_bytes(b"not canonical")
                try:
                    with self.assertRaises(ValueError):
                        canonical_inventory(self.source)
                finally:
                    path.unlink()
        self.temporary.symlink_to(self.blob)
        with self.assertRaisesRegex(ValueError, "symlink"):
            canonical_inventory(self.source)


class MemoryTraceTests(unittest.TestCase):
    def test_both_actual_backends_required_and_cache_restart_needs_no_raw_load(self):
        cycles = {"first": {"backend_pid": 111, "backend_start_ticks": 10},
                  "restart": {"backend_pid": 222, "backend_start_ticks": 20}}
        tree = SimpleNamespace(sampled_backend_identities={(111, 10), (222, 20)})
        rows = [{"pid": 111, "phase": "journal.raw_records_loaded", "wall_time_ns": 1},
                {"pid": 222, "phase": "prepared.full_model_dropped", "wall_time_ns": 2}]
        with tempfile.TemporaryDirectory() as directory:
            trace = Path(directory) / "trace.jsonl"
            trace.write_text("".join(json.dumps(row) + "\n" for row in rows))
            proof = validate_memory_trace(trace, cycles, tree)
            self.assertTrue(proof["validated"])
            self.assertFalse(proof["restart_raw_load_required"])
            for invalid in (rows[:1], rows[1:], rows + [{"pid": 333, "phase": "x", "wall_time_ns": 3}]):
                trace.write_text("".join(json.dumps(row) + "\n" for row in invalid))
                with self.assertRaises(ValueError):
                    validate_memory_trace(trace, cycles, tree)
            trace.write_text("invalid JSON\n")
            with self.assertRaises(ValueError):
                validate_memory_trace(trace, cycles, tree)
            trace.write_text("".join(json.dumps(row) + "\n" for row in rows))
            with self.assertRaisesRegex(ValueError, "not actually sampled"):
                validate_memory_trace(trace, cycles, SimpleNamespace(sampled_backend_identities=set()))

    def test_zombie_group_overwrite_preserves_observed_backend_identity(self):
        # Exercise PrivateTree's actual memory of classified samples while the
        # shared sampler's final rows lose /proc/exe at process teardown.
        tree = PrivateTree.__new__(PrivateTree)
        tree.last = {}
        tree.sampled_backend_identities = set()
        observed = iter((
            {"pid": 111, "start_ticks": 10, "group": "backend"},
            {"pid": 222, "start_ticks": 20, "group": "backend"},
            {"pid": 111, "start_ticks": 10, "group": "controller"},
            {"pid": 222, "start_ticks": 20, "group": "controller"}))

        def sampled(_phase=None):
            row = next(observed)
            tree.last[(row["pid"], row["start_ticks"])] = dict(row)
            return {"processes": [row]}

        with patch("clone_read.Tree.sample", side_effect=sampled):
            for _ in range(4):
                tree.sample()
        self.assertTrue(all(row["group"] == "controller" for row in tree.last.values()))
        self.assertEqual(tree.sampled_backend_identities, {(111, 10), (222, 20)})
        cycles = {"first": {"backend_pid": 111, "backend_start_ticks": 10},
                  "restart": {"backend_pid": 222, "backend_start_ticks": 20}}
        with tempfile.TemporaryDirectory() as directory:
            trace = Path(directory) / "trace.jsonl"
            trace.write_text(json.dumps({"pid": 111, "phase": "journal.raw_records_loaded", "wall_time_ns": 1}) + "\n"
                             + json.dumps({"pid": 222, "phase": "prepared.full_model_dropped", "wall_time_ns": 2}) + "\n")
            self.assertTrue(validate_memory_trace(trace, cycles, tree)["validated"])
            cycles["restart"]["backend_start_ticks"] = 21
            with self.assertRaisesRegex(ValueError, "not actually sampled"):
                validate_memory_trace(trace, cycles, tree)


if __name__ == "__main__":
    unittest.main()

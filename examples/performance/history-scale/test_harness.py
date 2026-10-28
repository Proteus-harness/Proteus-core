"""Bounded unit/exec checks, without Core builds, GUI or user sessions."""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest

from journal import digest, message_digest, seed_summary, verify_active
from launcher import MANIFEST_ENV, plan
from resume_idle import check_snapshot
from fixtures import prepare_case, prepare_fixtures
from measurement import Metrics

HERE = Path(__file__).resolve().parent


class LauncherTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        self.workspace = self.root / "workspace"
        self.workspace.mkdir()
        self.config = self.root / "config/configs/peer.toml"
        self.config.parent.mkdir(parents=True)
        self.config.write_text("# synthetic\n")
        self.session = self.root / "config/sessions/workspace/0000000001"
        self.session.mkdir(parents=True)
        (self.session / "journal.jsonl").write_text("{}\n")
        self.binary = self.root / "fake-core"
        self.binary.write_text(f"#!{sys.executable}\nimport os,sys,json\nprint(json.dumps(dict(pid=os.getpid(),argv=sys.argv)))\n")
        self.binary.chmod(0o755)
        self.manifest = dict(version=1, binary=str(self.binary), workspace=str(self.workspace),
                             entries={str(self.config): dict(session_dir=str(self.session))},
                             launch_log=str(self.root / "launch.jsonl"))
        self.argv = ["--config", str(self.config), "--cwd", str(self.workspace), "--new-session", "server", "stdio"]

    def tearDown(self):
        self.temporary.cleanup()

    def test_exact_exec_reuses_pid_and_preserves_stdout(self):
        path = self.root / "manifest.json"
        path.write_text(json.dumps(self.manifest))
        process = subprocess.run([sys.executable, str(HERE / "launcher.py"), *self.argv],
                                 env=dict(os.environ, **{MANIFEST_ENV: str(path)}),
                                 capture_output=True, text=True, check=True, timeout=5)
        output = json.loads(process.stdout)
        launch = json.loads((self.root / "launch.jsonl").read_text())
        self.assertEqual(output["pid"], launch["pid"])
        self.assertEqual(output["argv"], launch["argv"])
        self.assertNotIn("--new-session", output["argv"])
        self.assertEqual(output["argv"].count("--resume-session"), 1)
        self.assertEqual(output["argv"][-2:], ["server", "stdio"])

    def test_rejects_duplicate_flags_extra_arguments_and_unknown_configs(self):
        for argv in (self.argv + ["--new-session"], self.argv + ["extra"],
                     self.argv[:4] + ["--resume-session", str(self.session), "server", "stdio"],
                     ["--config", str(self.root / "not-allowlisted.toml"), *self.argv[2:]]):
            with self.subTest(argv=argv), self.assertRaises((ValueError, FileNotFoundError)):
                plan(argv, self.manifest)
        manifest = copy.deepcopy(self.manifest)
        manifest["workspace"] = str(self.root)
        with self.assertRaises(ValueError):
            plan(self.argv, manifest)
        manifest = copy.deepcopy(self.manifest)
        manifest["entries"][str(self.config)]["session_dir"] = str(self.workspace)
        with self.assertRaises(ValueError):
            plan(self.argv, manifest)


def message(index):
    return dict(id=f"message-{index}", role="User" if index % 2 == 0 else "Assistant",
                parts=[dict(part_id=f"part-{index}", provenance="user", scope="conversation",
                            payload={"Text": dict(text=f"Fixture row {index}\nСинтетический текст для замера памяти и прокрутки. ")})],
                metadata=None)


def inline(kind, payload, seq, turn=None):
    return dict(schema_version=21, session_seq=seq, session_id="synthetic-session", turn_id=turn,
                kind=kind, payload=dict(storage="inline", value=payload))


class PreservationTests(unittest.TestCase):
    def test_fixture_spec_keeps_uncompacted_input_separate(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            workspace = root / "workspace"
            workspace.mkdir()
            payload = workspace / "payload.txt"
            payload.write_text("synthetic")
            stored = dict(spec=dict(version=1, messages=[8], peers=1, text_repeats=1, payload_kib=1),
                          workspace=str(workspace), payload_sha256=digest(payload), sessions=[])
            (root / "fixtures.json").write_text(json.dumps(stored))
            args = SimpleNamespace(fixtures=root, messages=[8], peers=1, text_repeats=1, payload_kib=1,
                                   model_history_rows=None)
            self.assertEqual(prepare_fixtures(args), stored)
            args.model_history_rows = 3
            with self.assertRaisesRegex(ValueError, "specification differs"):
                prepare_fixtures(args)

    def test_compacted_seed_keeps_full_display_and_exact_active_tail(self):
        with tempfile.TemporaryDirectory() as temporary:
            session = Path(temporary)
            messages = [message(index) for index in range(8)]
            report = dict(changed=True, input_messages=8, output_messages=3,
                          summary_source="history-scale-fixture-tail", summary=None)
            initial = inline("history_mutated", dict(mutation="append", messages=messages, compaction=None), 1)
            compact = inline("history_mutated", dict(mutation="replace", messages=messages[-3:], compaction=report), 2)
            journal = session / "journal.jsonl"
            def write(rows):
                journal.write_text("".join(json.dumps(row) + "\n" for row in rows))
            write([initial, compact])
            seed = seed_summary(session, 8, 1, model_history_rows=3)
            self.assertEqual((seed["display_rows"], seed["model_history_rows"]), (8, 3))
            self.assertEqual(len(seed["ids"]), 8)
            self.assertEqual(seed["model_ids"], [message["id"] for message in messages[-3:]])
            with self.assertRaisesRegex(ValueError, "declared synthetic tail"):
                seed_summary(session, 8, 1)
            write([initial])
            with self.assertRaisesRegex(ValueError, "compaction is missing"):
                seed_summary(session, 8, 1, model_history_rows=3)
            extra = dict(id="current-user", role="User", parts=[], metadata=None)
            request = dict(request=dict(messages=copy.deepcopy(messages[-3:]) + [extra]))
            rows = [initial, compact,
                    inline("model_request_recorded", request, 3, "turn-1"),
                    inline("model_request_recorded", request, 4, "turn-1"),
                    inline("history_mutated", dict(mutation="checkpoint", messages=messages[-3:] + [extra], compaction=None), 5, "turn-1"),
                    inline("turn_settled", dict(status="success"), 6, "turn-1")]
            write(rows)
            verified = verify_active(session, seed, 1)
            self.assertEqual((verified["display_rows"], verified["model_history_rows"]), (8, 3))
            self.assertEqual(verified["first_requests"]["turn-1"]["seeded_messages"], 3)
            changed = copy.deepcopy(rows)
            changed[2]["payload"]["value"]["request"]["messages"].insert(0, message(0))
            write(changed)
            with self.assertRaisesRegex(ValueError, "model request lost"):
                verify_active(session, seed, 1)
            changed = copy.deepcopy(rows)
            changed[4]["payload"]["value"]["mutation"] = "replace"
            write(changed)
            with self.assertRaisesRegex(ValueError, "reset persistent UI"):
                verify_active(session, seed, 1)

    def test_readonly_hardlinks_and_active_rejection(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source/sessions/workspace/seed"
            source.mkdir(parents=True)
            journal = source / "journal.jsonl"
            journal.write_text("synthetic canonical input\n")
            summary = root / "seed.json"
            summary.write_text(json.dumps(dict(session_id="seed", tail=[])))
            fixtures = dict(workspace=str(root), spec=dict(model_history_rows=None), sessions=[dict(messages=1, peer=0, session_dir=str(source),
                            relative_session="sessions/workspace/seed", seed_summary=str(summary),
                            canonical_files={"journal.jsonl": digest(journal)})])
            args = SimpleNamespace(link_fixtures=True, mode="resume-idle", module=Path(sys.executable),
                                   binary=Path(sys.executable), delay_ms=500, rounds=1, peers=1)
            manifest = prepare_case(args, fixtures, root / "case", 1)
            copied = Path(next(iter(manifest["entries"].values()))["session_dir"]) / "journal.jsonl"
            self.assertEqual(journal.stat().st_ino, copied.stat().st_ino)
            args.mode = "agent-control"
            with self.assertRaisesRegex(ValueError, "forbidden"):
                prepare_case(args, fixtures, root / "active-case", 1)

    def test_idle_cpu_uses_weighted_interval_mean(self):
        metrics = Metrics.__new__(Metrics)
        metrics.tree = SimpleNamespace(last={}, clock_ticks=100, totals=lambda: {})
        metrics.small = []
        for elapsed, ticks in ((0.1, 10), (0.9, 0)):
            metrics.small.append(dict(phase="idle", interval_seconds=elapsed, rss_kib=10, pss_kib=8,
                pss_swap_kib=9, swap_pss_kib=1, private_kib=4, cpu_percent_one_core=100 * ticks / 100 / elapsed,
                interval_counters=dict(cpu_ticks=ticks, read_bytes=0, write_bytes=0, rchar=0, wchar=0)))
        phase = metrics.summary()["phases"]["idle"]
        self.assertAlmostEqual(phase["cpu_percent_one_core_weighted_mean"], 10)
        self.assertEqual(phase["swap_pss_kib"]["median"], 1)

    def test_snapshot_full_baseline_and_paged_current_agree(self):
        messages = [dict(message_id=f"message-{index}", text=f"row {index}") for index in range(100)]
        entry = dict(session_id="seed", rows=100, tail=messages[-50:])
        baseline = dict(session_id="seed", transcript=messages)
        current = dict(session_id="seed", transcript=messages[-50:], history=dict(before=50))
        self.assertEqual(check_snapshot(baseline, entry)["total_messages"], 100)
        self.assertEqual(check_snapshot(current, entry)["total_messages"], 100)
        current["transcript"][-1] = dict(message_id="changed", text="row 99")
        with self.assertRaises(ValueError):
            check_snapshot(current, entry)

    def test_seed_preservation_active_requests_and_negative_controls(self):
        with tempfile.TemporaryDirectory() as temporary:
            session = Path(temporary)
            messages = [message(index) for index in range(4)]
            initial = inline("history_mutated", dict(mutation="append", messages=messages, compaction=None), 1)
            journal = session / "journal.jsonl"
            journal.write_text(json.dumps(initial) + "\n")
            seed = seed_summary(session, 4, 1)
            self.assertEqual(seed["messages_sha256"], message_digest(messages)[1])
            extra = dict(id="current-user", role="User", parts=[], metadata=None)
            request = dict(request=dict(messages=copy.deepcopy(messages) + [extra]))
            records = [initial,
                       inline("model_request_recorded", request, 2, "turn-1"),
                       inline("model_request_recorded", request, 3, "turn-1"),
                       inline("history_mutated", dict(mutation="checkpoint", messages=messages + [extra], compaction=None), 4, "turn-1"),
                       inline("turn_settled", dict(status="success"), 5, "turn-1")]
            def write(rows):
                journal.write_text("".join(json.dumps(row) + "\n" for row in rows))
            write(records)
            self.assertEqual(verify_active(session, seed, 1)["seeded_messages"], 4)
            changed = copy.deepcopy(records)
            changed[1]["payload"]["value"]["request"]["messages"][0]["id"] = "lost"
            write(changed)
            with self.assertRaisesRegex(ValueError, "model request lost"):
                verify_active(session, seed, 1)
            changed = copy.deepcopy(records)
            changed[3]["payload"]["value"]["messages"] = [extra]
            write(changed)
            with self.assertRaisesRegex(ValueError, "discarded"):
                verify_active(session, seed, 1)
            changed = copy.deepcopy(records)
            changed[3]["payload"]["value"]["compaction"] = dict(changed=True)
            write(changed)
            with self.assertRaisesRegex(ValueError, "compacted"):
                verify_active(session, seed, 1)

    def test_blob_hash_negative_control(self):
        with tempfile.TemporaryDirectory() as temporary:
            session = Path(temporary)
            payload = dict(mutation="append", messages=[message(0)], compaction=None)
            (session / "blobs").mkdir()
            blob = session / "blobs/temp.json"
            blob.write_text(json.dumps(payload))
            sha = digest(blob)
            blob = blob.rename(session / "blobs" / f"{sha}.json")
            record = inline("history_mutated", payload, 1)
            record["payload"] = dict(storage="blob", sha256=sha, bytes=blob.stat().st_size,
                                     relative_path=f"blobs/{sha}.json")
            (session / "journal.jsonl").write_text(json.dumps(record) + "\n")
            self.assertEqual(seed_summary(session, 1, 1)["rows"], 1)
            blob.write_text("corrupted")
            with self.assertRaisesRegex(ValueError, "size/hash mismatch"):
                seed_summary(session, 1, 1)


if __name__ == "__main__":
    unittest.main()

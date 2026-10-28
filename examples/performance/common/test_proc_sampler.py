"""Small self-contained checks; no existing user process is sampled."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

if __package__:
    from .proc_sampler import Recorder, Tree
else:
    from proc_sampler import Recorder, Tree


class SamplerTests(unittest.TestCase):
    def fake_process(self, proc, pid, parent, start, rss, pss, cpu=0):
        path = proc / str(pid)
        path.mkdir(exist_ok=True)
        fields = ["0"] * 22
        fields[0], fields[1], fields[11], fields[12], fields[19] = "S", str(parent), str(cpu), "0", str(start)
        (path / "stat").write_text(f"{pid} (name with ) parens) " + " ".join(fields))
        (path / "smaps_rollup").write_text(f"Rss: {rss} kB\nPss: {pss} kB\nSwapPss: 2 kB\nPrivate_Clean: 3 kB\nPrivate_Dirty: 4 kB\n")
        (path / "io").write_text("read_bytes: 10\nwrite_bytes: 20\nrchar: 30\nwchar: 40\n")
        (path / "status").write_text("VmHWM: 10000 kB\nThreads: 1\n")
        (path / "cmdline").write_bytes(b"/test/fake-backend\0")

    def test_retained_children_reuse_and_simultaneous_peaks(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as temporary:
            proc = Path(temporary)
            (proc / "uptime").write_text("1000.0 0\n")
            self.fake_process(proc, 100, 1, 100, 100, 80)
            self.fake_process(proc, 101, 100, 200, 200, 100)
            tree = Tree(100, proc=proc)
            first = tree.sample("first")
            self.assertEqual(first["rss_kib"], 300)
            self.assertEqual(first["pss_swap_kib"], 184)
            self.assertEqual(first["groups"]["backend"]["process_count"], 1)
            self.fake_process(proc, 100, 1, 100, 150, 120, 2)
            self.fake_process(proc, 101, 100, 200, 100, 50, 4)
            second = tree.sample("second")
            self.assertEqual(second["interval_counters"]["cpu_ticks"], 6)
            self.assertEqual(tree.totals()["sampled_simultaneous_peaks_kib"]["rss_kib"], 300)
            # Reparent the discovered child; the original root disappears.
            for item in (proc / "100").iterdir():
                item.unlink()
            (proc / "100").rmdir()
            self.fake_process(proc, 101, 1, 200, 100, 50, 5)
            self.assertEqual(tree.sample()["process_count"], 1)
            # A reused root PID and a reused retained-child PID are not owned.
            self.fake_process(proc, 100, 1, 999, 999, 999)
            self.fake_process(proc, 101, 100, 999, 999, 999)
            self.fake_process(proc, 102, 100, 999, 999, 999)
            final = tree.sample()
            self.assertEqual(final["process_count"], 0)
            self.assertEqual(final["rss_kib"], 0)
            self.assertEqual(tree.totals()["observed_processes"], 2)
            self.assertEqual(tree.totals()["cpu_ticks"], 7)

    def test_jsonl_markers_and_missing_memory(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as temporary:
            proc = Path(temporary)
            (proc / "uptime").write_text("1000.0 0\n")
            self.fake_process(proc, 100, 1, 100, 100, 80)
            (proc / "100" / "smaps_rollup").unlink()
            tree = Tree(100, proc=proc)
            with Recorder(tree, proc / "test.jsonl") as recorder:
                recorder.mark("startup", fixture=5000)
                event = recorder.sample()
                self.assertEqual(event["memory_incomplete_pids"], [100])
            events = [json.loads(line) for line in (proc / "test.jsonl").read_text().splitlines()]
            self.assertEqual([event["type"] for event in events], ["phase", "sample", "summary"])
            self.assertEqual(events[1]["phase"], "startup")

    def test_live_synthetic_reparent_and_exit(self):
        # 8 MiB in the root + 8 MiB in its child, no application or GUI launch.
        child_code = "import time; data=bytearray(8*1024*1024); print('child_ready', flush=True); time.sleep(30)"
        parent_code = (
            "import subprocess, sys, time; data=bytearray(8*1024*1024); "
            f"child=subprocess.Popen([sys.executable, '-u', '-c', {child_code!r}]); "
            "print('parent_ready', child.pid, flush=True); time.sleep(30)"
        )
        process = subprocess.Popen([sys.executable, "-u", "-c", parent_code], stdout=subprocess.PIPE, text=True)
        child_pid = None
        try:
            for _ in range(2):
                line = process.stdout.readline()
                if line.startswith("parent_ready"):
                    child_pid = int(line.split()[1])
            self.assertIsNotNone(child_pid)
            tree = Tree(process.pid)
            initial = tree.sample()
            self.assertEqual(initial["process_count"], 2)
            self.assertFalse(initial["memory_incomplete_pids"])
            self.assertGreater(initial["pss_kib"], 16 * 1024)
            process.terminate()
            process.wait(timeout=5)
            orphan = tree.sample()
            self.assertEqual([row["pid"] for row in orphan["processes"]], [child_pid])
            os.kill(child_pid, signal.SIGTERM)
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                ended = tree.sample()
                if ended["rss_kib"] == 0:
                    break
                time.sleep(0.01)
            self.assertEqual(ended["rss_kib"], 0)
            self.assertEqual(ended["pss_swap_kib"], 0)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            if child_pid is not None:
                try:
                    os.kill(child_pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            process.stdout.close()

    def test_transient_io_failure_does_not_inflate_next_interval(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as temporary:
            proc = Path(temporary)
            (proc / "uptime").write_text("1000.0 0\n")
            self.fake_process(proc, 100, 1, 100, 100, 80)
            tree = Tree(100, proc=proc)
            tree.sample()
            (proc / "100" / "io").unlink()
            tree.sample()
            self.assertEqual(tree.totals()["read_bytes"], 10)
            (proc / "100" / "io").write_text("read_bytes: 100\nwrite_bytes: 200\n")
            recovery = tree.sample()
            self.assertEqual(recovery["interval_counters"]["read_bytes"], 0)
            self.assertIn("read_bytes", recovery["processes"][0]["counter_missing_baselines"])
            self.assertEqual(tree.totals()["read_bytes"], 100)


if __name__ == "__main__":
    unittest.main()

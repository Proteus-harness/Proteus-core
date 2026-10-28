"""Linux process-tree sampler; memory is live, counters are observed lifetime totals.

All sizes are KiB; CPU percent is percent of ONE logical CPU (100 = one core).
No background threads or global process/signal state are installed on import.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import time
from typing import Callable


MEMORY_KEYS = ("Rss", "Pss", "SwapPss", "Private_Clean", "Private_Dirty")
IO_KEYS = ("rchar", "wchar", "syscr", "syscw", "read_bytes", "write_bytes", "cancelled_write_bytes")
COUNTERS = ("cpu_ticks", "minor_faults", "major_faults") + IO_KEYS
GROUPS = ("gui", "backend", "modules", "extensions", "other")


def read_stat(path: Path) -> dict:
    raw = (path / "stat").read_text()
    end = raw.rindex(")")
    values = raw[end + 2:].split()
    return {
        "pid": int(path.name), "name": raw[raw.index("(") + 1:end],
        "state": values[0], "parent": int(values[1]),
        "start_ticks": int(values[19]), "cpu_ticks": int(values[11]) + int(values[12]),
        "minor_faults": int(values[7]), "major_faults": int(values[9]),
    }


def table(proc: Path = Path("/proc")) -> dict[int, dict]:
    result = {}
    for path in proc.iterdir():
        if path.name.isdigit():
            try:
                result[int(path.name)] = read_stat(path)
            except (OSError, ValueError, IndexError):
                pass
    return result


def default_group(row: dict) -> str:
    """Conservative executable classification; pass a callback for exact launchers.

    Only executable/argv[0] is inspected: an extension path passed as an argument
    must not accidentally classify the parent GUI as an extension.
    """
    names = [Path(row.get("exe", "")).name.lower()]
    if row.get("argv"):
        names.append(Path(row["argv"][0]).name.lower())
    if any("extension" in name for name in names):
        return "extensions"
    if any("module" in name for name in names):
        return "modules"
    if any("backend" in name or "proteus-core" in name for name in names):
        return "backend"
    # QtWebEngine/WebKit/GPU helper processes belong to the GUI footprint.
    if any("proteus-desktop" in name or "qtwebengine" in name or "webkit" in name
           for name in names):
        return "gui"
    return "other"


class Tree:
    def __init__(self, root: int, *, classifier: Callable[[dict], str] | None = None,
                 root_group: str = "gui", proc: Path | str = Path("/proc")):
        self.root = int(root)
        self.proc = Path(proc)
        self.root_start = read_stat(self.proc / str(root))["start_ticks"]
        self.classifier = classifier or default_group
        self.root_group = root_group
        self.clock_ticks = os.sysconf("SC_CLK_TCK")
        self.owned = {self.root: self.root_start}
        self.last: dict[tuple[int, int], dict] = {}
        self.last_sequence: dict[tuple[int, int], int] = {}
        self.last_counter_fields: dict[tuple[int, int], set[str]] = {}
        self.previous_ns: int | None = None
        self.previous_boot_ticks: float | None = None
        self.previous_alive: set[tuple[int, int]] = set()
        self.sample_count = 0
        self.peaks = {"pss_kib": 0, "pss_swap_kib": 0, "rss_kib": 0}
        self.peak_samples: dict[str, dict] = {}

    def _observe(self, pid: int, start: int) -> dict | None:
        path = self.proc / str(pid)
        try:
            row = read_stat(path)
            if row["start_ticks"] != start:
                return None
            row["errors"] = {}
            for field in MEMORY_KEYS:
                row[field] = None
            for filename in ("smaps_rollup", "io", "status"):
                try:
                    for line in (path / filename).read_text().splitlines():
                        key, sep, value = line.partition(":")
                        if sep and (key in MEMORY_KEYS or key in IO_KEYS or key in ("VmHWM", "Threads")):
                            row[key] = int(value.split()[0])
                except (OSError, ValueError) as error:
                    row["errors"][filename] = str(error)
            try:
                row["exe"] = os.readlink(path / "exe")
            except OSError:
                row["exe"] = ""
            try:
                row["argv"] = (path / "cmdline").read_bytes().decode(errors="replace").rstrip("\0").split("\0")
            except OSError:
                row["argv"] = []
            # PID may exit/recycle while smaps/io/status are read. Reject the
            # entire mixed observation. Zombies have no resident address space.
            after = read_stat(path)
            if after["start_ticks"] != start:
                return None
            row.update(after)
            if row["state"] in ("Z", "X", "x"):
                row.update({field: 0 for field in MEMORY_KEYS})
            row["group"] = self.root_group if pid == self.root and start == self.root_start else self.classifier(row)
            return row
        except (OSError, ValueError, IndexError):
            return None

    def sample(self, phase: str | None = None) -> dict:
        started_ns = time.monotonic_ns()
        wall_ns = time.time_ns()
        try:
            boot_ticks = float((self.proc / "uptime").read_text().split()[0]) * self.clock_ticks
        except (OSError, ValueError, IndexError):
            boot_ticks = None
        current = table(self.proc)
        roots = {pid for pid, start in self.owned.items()
                 if pid in current and current[pid]["start_ticks"] == start}
        # No unconditional root insertion: a recycled root PID must never be owned.
        while True:
            more = {pid for pid, row in current.items() if row["parent"] in roots} - roots
            if not more:
                break
            roots.update(more)
        for pid in roots:
            self.owned[pid] = current[pid]["start_ticks"]
        processes = []
        deltas = {field: 0 for field in COUNTERS}
        cpu_missing_baselines = []
        alive = set()
        elapsed = None if self.previous_ns is None else (started_ns - self.previous_ns) / 1e9
        for pid in sorted(roots):
            row = self._observe(pid, current[pid]["start_ticks"])
            if row is None:
                continue
            identity = (pid, row["start_ticks"])
            lifetime_prior = self.last.get(identity)
            prior = lifetime_prior if self.last_sequence.get(identity) == self.sample_count - 1 else None
            born_in_interval = (self.previous_boot_ticks is not None and
                                row["start_ticks"] >= self.previous_boot_ticks)
            row_deltas = {field: 0 for field in COUNTERS}
            row["counter_missing_baselines"] = []
            if self.previous_ns is not None:
                if prior is not None or born_in_interval:
                    for field in COUNTERS:
                        if field in row and (prior is None or field in self.last_counter_fields[identity]):
                            row_deltas[field] = max(0, row[field] - (prior or {}).get(field, 0))
                            deltas[field] += row_deltas[field]
                        elif field in row:
                            row["counter_missing_baselines"].append(field)
                else:
                    cpu_missing_baselines.append(pid)
                    row["counter_missing_baselines"] = [field for field in COUNTERS if field in row]
            row["interval_counters"] = row_deltas if elapsed is not None else None
            row["interval_cpu_percent_one_core"] = (100 * row_deltas["cpu_ticks"] / self.clock_ticks / elapsed
                                                     if elapsed else None)
            # Preserve last observed cumulative counters across a permission/read
            # failure, but never reuse old resident-memory values in live totals.
            retained = dict(row)
            for field in COUNTERS + ("VmHWM",):
                if field not in retained and lifetime_prior is not None and field in lifetime_prior:
                    retained[field] = lifetime_prior[field]
            self.last[identity] = retained
            self.last_sequence[identity] = self.sample_count
            self.last_counter_fields[identity] = {field for field in COUNTERS if field in row}
            processes.append(row)
            alive.add(identity)
        cpu_percent = None if not elapsed else 100 * deltas["cpu_ticks"] / self.clock_ticks / elapsed
        groups = {}
        for group in sorted(set(GROUPS) | {row["group"] for row in processes}):
            rows = [row for row in processes if row["group"] == group]
            groups[group] = self._memory_totals(rows)
            groups[group]["process_count"] = len(rows)
            groups[group]["interval_counters"] = ({field: sum((row["interval_counters"] or {}).get(field, 0)
                                                               for row in rows) for field in COUNTERS}
                                                    if elapsed is not None else None)
            groups[group]["interval_cpu_percent_one_core"] = (sum(row["interval_cpu_percent_one_core"] or 0
                                                                    for row in rows) if elapsed else None)
        result = {
            "type": "sample", "sequence": self.sample_count, "phase": phase,
            "monotonic_ns": started_ns, "wall_time_ns": wall_ns,
            "root_pid": self.root, "root_start_ticks": self.root_start,
            "processes": processes, "groups": groups,
            **self._memory_totals(processes), "process_count": len(processes),
            "interval_seconds": elapsed, "interval_cpu_percent_one_core": cpu_percent,
            "interval_counters": deltas if elapsed is not None else None,
            "cpu_missing_baseline_pids": cpu_missing_baselines,
            "disappeared_since_previous": [dict(pid=pid, start_ticks=start)
                                           for pid, start in sorted(self.previous_alive - alive)],
            "scan_duration_seconds": (time.monotonic_ns() - started_ns) / 1e9,
        }
        for key in self.peaks:
            if result[key] >= self.peaks[key]:
                self.peaks[key] = result[key]
                self.peak_samples[key] = dict(sequence=self.sample_count, monotonic_ns=started_ns, phase=phase)
        self.sample_count += 1
        self.previous_ns, self.previous_boot_ticks, self.previous_alive = started_ns, boot_ticks, alive
        return result

    @staticmethod
    def _memory_totals(rows: list[dict]) -> dict:
        def total(field):
            return sum(row.get(field) or 0 for row in rows)
        return {
            "rss_kib": total("Rss"), "pss_kib": total("Pss"),
            "pss_swap_kib": total("Pss") + total("SwapPss"),
            "swap_pss_kib": total("SwapPss"),
            "private_kib": total("Private_Clean") + total("Private_Dirty"),
            "private_clean_kib": total("Private_Clean"),
            "private_dirty_kib": total("Private_Dirty"),
            "memory_incomplete_pids": [row["pid"] for row in rows
                                       if any(row.get(field) is None for field in MEMORY_KEYS)],
        }

    def totals(self) -> dict:
        counters = {field: sum(row.get(field, 0) for row in self.last.values()) for field in COUNTERS}
        return {
            **counters, "cpu_seconds": counters["cpu_ticks"] / self.clock_ticks,
            "observed_processes": len(self.last), "samples": self.sample_count,
            "sampled_simultaneous_peaks_kib": dict(self.peaks),
            "peak_samples": dict(self.peak_samples),
            # VmHWM is per process and intentionally never summed as a tree peak.
            "per_process_vm_hwm_kib": [{"pid": pid, "start_ticks": start, "VmHWM": row.get("VmHWM")}
                                       for (pid, start), row in sorted(self.last.items())],
            "counter_scope": "last observed per identity; final unsampled CPU/IO and unseen short-lived children are omitted",
        }


class Recorder:
    """Flush each JSONL event so a failed benchmark retains its partial evidence."""
    def __init__(self, tree: Tree, path: Path | str):
        self.tree = tree
        self.file = open(path, "w", encoding="utf-8")
        self.phase: str | None = None

    def write(self, event: dict) -> dict:
        self.file.write(json.dumps(event, ensure_ascii=False) + "\n")
        self.file.flush()
        return event

    def mark(self, phase: str, **details) -> dict:
        self.phase = phase
        return self.write({"type": "phase", "phase": phase, "details": details,
                           "monotonic_ns": time.monotonic_ns(), "wall_time_ns": time.time_ns()})

    def sample(self) -> dict:
        return self.write(self.tree.sample(self.phase))

    def close(self):
        if not self.file.closed:
            self.write({"type": "summary", "monotonic_ns": time.monotonic_ns(), **self.tree.totals()})
            self.file.close()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pid", type=int)
    parser.add_argument("--output", required=True)
    parser.add_argument("--duration", type=float, default=10)
    parser.add_argument("--interval", type=float, default=0.1)
    parser.add_argument("--phase", default="measurement")
    args = parser.parse_args()
    if args.duration < 0 or args.interval <= 0:
        parser.error("duration must be nonnegative and interval must be positive")
    tree = Tree(args.pid)
    deadline = time.monotonic() + args.duration
    with Recorder(tree, args.output) as recorder:
        recorder.mark(args.phase)
        while True:
            recorder.sample()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            time.sleep(min(args.interval, remaining))


if __name__ == "__main__":
    main()

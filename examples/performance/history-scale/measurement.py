"""Sampling/control helpers shared by read-only and active history workloads."""
import json
import os
from pathlib import Path
import signal
import statistics
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "common"))
from proc_sampler import PHASE_INTERVAL_POLICY, Recorder, Tree, table


def classifier(args):
    backend = str(args.binary)
    modules = (str(args.module), str(args.storage))

    def classify(row):
        executable = row.get("exe", "")
        if executable == backend:
            return "backend"
        if executable in modules:
            return "modules"
        # The short Python exec adapter belongs to launch overhead, not Core.
        return "controller"
    return classify


class Metrics:
    def __init__(self, tree, case, args):
        self.tree, self.args = tree, args
        self.log = Recorder(tree, case / "samples.jsonl")
        self.small = []
        self.started = time.monotonic()

    def mark(self, phase, **details):
        return self.log.mark(phase, **details)

    def sample(self):
        event = self.tree.sample(self.log.phase)
        rows = [row for row in event["processes"] if row["group"] in ("backend", "modules")]
        subject = dict(Tree._memory_totals(rows), process_count=len(rows),
                       cpu_percent_one_core=sum(row["interval_cpu_percent_one_core"] or 0 for row in rows))
        subject["interval_counters"] = {field: sum((row["interval_counters"] or {}).get(field, 0) for row in rows)
                                        for field in ("cpu_ticks", "read_bytes", "write_bytes", "rchar", "wchar")}
        event["subject"] = subject
        self.log.write(event)
        # Only the exact configured controller root has a known exclusion
        # without an observed row. A non-root can have exec'd into the subject.
        controller = ((event["root_pid"], event["root_start_ticks"])
                      if self.tree.root_group == "controller" else None)
        unobserved = [item for item in event["memory_unobserved_owned"]
                      if (item["pid"], item["start_ticks"]) != controller]
        if not unobserved:
            self.small.append(dict(phase=event["phase"], monotonic_ns=event["monotonic_ns"],
                                   interval_seconds=event["interval_seconds"],
                                   phase_interval_eligible=event["phase_interval_eligible"], **subject))
        if subject["pss_swap_kib"] > self.args.max_pss_mib * 1024:
            raise MemoryError(f"subject PSS+SwapPss exceeded {self.args.max_pss_mib} MiB")
        if subject["memory_incomplete_pids"]:
            raise RuntimeError(f"incomplete subject memory sample: {subject['memory_incomplete_pids']}")
        if unobserved:
            raise RuntimeError(f"unobserved owned processes: {unobserved}")
        if time.monotonic() - self.started > self.args.timeout:
            raise TimeoutError(f"case exceeded {self.args.timeout} seconds")
        return subject

    def pause(self, seconds, processes):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if any(process.poll() is not None for process in processes):
                raise RuntimeError("a measured process exited during resident idle")
            self.sample()
            time.sleep(min(self.args.interval, max(0, deadline - time.monotonic())))

    def summary(self):
        phases = {}
        for phase in sorted({sample["phase"] for sample in self.small}):
            samples = [sample for sample in self.small if sample["phase"] == phase]
            phases[phase] = dict(samples=len(samples))
            for key in ("rss_kib", "pss_kib", "pss_swap_kib", "swap_pss_kib", "private_kib"):
                values = [sample[key] for sample in samples]
                phases[phase][key] = dict(median=statistics.median(values), peak=max(values))
            intervals = [sample for sample in samples if sample.get("phase_interval_eligible") is True]
            excluded = [sample for sample in samples if sample.get("phase_interval_eligible") is not True]
            cpu = [sample["cpu_percent_one_core"] for sample in intervals]
            phases[phase]["cpu_percent_one_core"] = dict(median=statistics.median(cpu) if cpu else None,
                                                        peak=max(cpu) if cpu else None)
            phases[phase]["eligible_interval_samples"] = len(intervals)
            phases[phase]["excluded_interval_samples"] = len(excluded)
            phases[phase]["excluded_interval_seconds"] = sum(sample["interval_seconds"] or 0 for sample in excluded)
            phases[phase]["excluded_interval_counters"] = {
                field: sum(sample["interval_counters"][field] for sample in excluded)
                for field in ("cpu_ticks", "read_bytes", "write_bytes", "rchar", "wchar")}
            elapsed = sum(sample["interval_seconds"] for sample in intervals)
            phase_counters = {field: sum(sample["interval_counters"][field] for sample in intervals)
                              for field in ("cpu_ticks", "read_bytes", "write_bytes", "rchar", "wchar")}
            phases[phase]["observed_interval_counters"] = phase_counters
            phases[phase]["observed_interval_seconds"] = elapsed
            phases[phase]["cpu_percent_one_core_weighted_mean"] = (
                100 * phase_counters["cpu_ticks"] / self.tree.clock_ticks / elapsed if elapsed else None)
        observed = [row for row in self.tree.last.values() if row["group"] in ("backend", "modules")]
        counters = {field: sum(row.get(field, 0) for row in observed)
                    for field in ("cpu_ticks", "read_bytes", "write_bytes", "rchar", "wchar", "minor_faults", "major_faults")}
        counters["cpu_seconds"] = counters["cpu_ticks"] / self.tree.clock_ticks
        return dict(phases=phases, phase_interval_policy=PHASE_INTERVAL_POLICY, subject_observed_counters=counters,
                    sampled_peak_pss_swap_kib=max((sample["pss_swap_kib"] for sample in self.small), default=0),
                    sampled_peak_rss_kib=max((sample["rss_kib"] for sample in self.small), default=0),
                    sampling_scope="actual Core backends + reference and selected session-storage modules; Python/driver control overhead separately grouped",
                    full_tree=self.tree.totals())

    def close(self):
        self.log.close()


def cleanup(processes, tree):
    """Only identities discovered under our measured root; never kill by name."""
    tree.sample()
    for sig in (signal.SIGTERM, signal.SIGKILL):
        current = table()
        alive = [pid for pid, start in tree.owned.items() if pid != os.getpid()
                 and pid in current and current[pid]["start_ticks"] == start
                 and current[pid]["state"] not in ("Z", "X", "x")]
        for pid in sorted(alive, reverse=True):
            try:
                os.kill(pid, sig)
            except ProcessLookupError:
                pass
        deadline = time.monotonic() + 3
        for process in processes:
            try:
                process.wait(timeout=max(0, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                pass
        if not alive:
            break
        time.sleep(0.05)
    current = table()
    survivors = [pid for pid, start in tree.owned.items() if pid != os.getpid() and pid in current
                 and current[pid]["start_ticks"] == start and current[pid]["state"] not in ("Z", "X", "x")]
    if survivors:
        raise RuntimeError(f"owned process identities survived cleanup: {survivors}")


def save_partial(case, metrics, error=None):
    value = metrics.summary()
    value.update(status="failed" if error else "measured", error=error)
    (case / "measurement.json").write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    return value

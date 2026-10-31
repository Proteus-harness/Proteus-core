#!/usr/bin/env python3
"""Private canonical session clone: read-only local HTTP cold/restart measurement."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import queue
import re
import signal
import subprocess
import threading
import time
from urllib.parse import quote, urlsplit
from urllib.request import ProxyHandler, Request, build_opener
import uuid
from types import SimpleNamespace

from fixtures import profile, write_json
from measurement import Metrics, Tree, classifier, cleanup, save_partial
from journal import digest


def opaque(value):
    return hashlib.sha256(str(value).encode()).hexdigest()


def encoded_digest(value):
    return opaque(json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")))


CANONICAL_BLOB = re.compile(r"[0-9a-f]{64}\.json")
WRITER_TEMP_BLOB = re.compile(
    r"\.[0-9a-f]{64}\.tmp\.[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")


def canonical_inventory(session):
    files = [session / "session.json", session / "journal.jsonl"]
    blobs = session / "blobs"
    if blobs.is_symlink():
        raise ValueError("canonical symlinks are unsupported")
    if blobs.exists():
        if not blobs.is_dir():
            raise ValueError("unexpected canonical blob layout")
        files.extend(sorted(blobs.iterdir()))
    result, temporary = {}, {}
    for path in files:
        if path.is_symlink():
            raise ValueError("canonical symlinks are unsupported")
        if not path.is_file() or not path.resolve().is_relative_to(session):
            raise ValueError("canonical input is not a regular in-session file")
        relative = str(path.relative_to(session))
        value = {"sha256": digest(path), "bytes": path.stat().st_size}
        if relative.startswith("blobs/"):
            if WRITER_TEMP_BLOB.fullmatch(path.name):
                # storage::write_blob creates exactly .<sha>.tmp.<record UUID>.
                # Keep its hash in private memory for immutability, never copy
                # it or expose the temporary filename/hash in the report.
                temporary[relative] = value
                continue
            if not CANONICAL_BLOB.fullmatch(path.name):
                raise ValueError("unexpected canonical blob layout")
        result[relative] = value
    return result, temporary


def canonical_files(session):
    return canonical_inventory(session)[0]


class PrivateTree(Tree):
    """Reuse the sampler; remove argv before its recorder can persist paths."""
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.sampled_backend_identities = set()

    def sample(self, phase=None):
        event = super().sample(phase)
        self.sampled_backend_identities.update(
            (row["pid"], row["start_ticks"]) for row in event["processes"]
            if row["group"] == "backend")
        for row in [*event["processes"], *self.last.values()]:
            row["argv"] = []
            row["argv_redacted"] = True
            row["errors"] = {key: "unavailable" for key in row.get("errors", {})}
        return event


def local_origin(value):
    url = urlsplit(value)
    if (url.scheme != "http" or url.hostname != "127.0.0.1" or not url.port
            or url.username or url.password or url.path or url.query or url.fragment):
        raise ValueError("unexpected HTTP origin")
    return value


def snapshot_summary(snapshot, expected_id):
    if snapshot.get("session_id") != expected_id:
        raise ValueError("snapshot session identity mismatch")
    if (snapshot.get("execution") or {}).get("active") is not None:
        raise ValueError("unexpected active execution")
    rows = snapshot["transcript"]
    before = (snapshot.get("history") or {}).get("before", 0)
    return {"session_id_sha256": opaque(expected_id), "visible_messages": len(rows),
            "history_before": before, "total_messages": before + len(rows),
            "transcript_sha256": encoded_digest(rows), "tail_sha256": encoded_digest(rows[-50:]),
            "active_execution": False}


def observe(process, session, expected_id, events, timeout):
    """Consume stdout/SSE in memory; persist neither text nor full snapshots."""
    try:
        origin = None
        for line in process.stdout:
            if line.startswith("{"):
                ready = json.loads(line)
                if ready.get("type") == "http_ready":
                    origin = local_origin(ready["origin"])
                    break
        if origin is None:
            raise RuntimeError("HTTP readiness missing")
        events.put({"type": "http_bound", "origin": origin,
                    "monotonic_ns": time.monotonic_ns(), "wall_time_ns": time.time_ns()})
        client = build_opener(ProxyHandler({}))
        with client.open(origin + "/health", timeout=timeout) as response:
            if json.loads(response.read(65536)).get("ok") is not True:
                raise ValueError("HTTP health failed")
        request = origin + "/events?session_dir=" + quote(str(session), safe="")
        with client.open(request, timeout=timeout) as response:
            while True:
                line = response.readline(64 * 1024 * 1024 + 1)
                if not line or len(line) > 64 * 1024 * 1024:
                    raise RuntimeError("snapshot stream ended or exceeded bound")
                if not line.startswith(b"data: "):
                    continue
                record = json.loads(line[6:])
                if record.get("type") == "response" and record.get("ok") is False:
                    raise RuntimeError("snapshot stream returned an error")
                event = record.get("event", {})
                if event.get("type") == "session_snapshot":
                    summary = snapshot_summary(event["snapshot"], expected_id)
                    events.put({"type": "snapshot_ready", "summary": summary,
                                "monotonic_ns": time.monotonic_ns(), "wall_time_ns": time.time_ns()})
                    del record, event, summary, line
                    break
        for line in process.stdout:
            pass  # Drain diagnostics without a stdout-content artifact.
    except Exception as exc:
        events.put({"type": "error", "error_type": type(exc).__name__})


def clone_source(args, private):
    spec = json.loads(args.source_spec.read_text())
    source = Path(spec["source_session_dir"]).resolve(strict=True)
    metadata = json.loads((source / "session.json").read_text())
    expected_id = str(uuid.UUID(metadata["session_id"]))
    workspace = Path(metadata["workspace_path"]).resolve(strict=True)
    if (metadata["schema_version"] != 4 or metadata["journal_schema_version"] != 21
            or source.name != f"{uuid.UUID(expected_id).int % 10_000_000_000:010d}"
            or not workspace.is_dir()):
        raise ValueError("unsupported source session identity")
    if spec.get("workspace_path") is not None and Path(spec["workspace_path"]).resolve(strict=True) != workspace:
        raise ValueError("source specification workspace mismatch")
    before, temporary_before = canonical_inventory(source)
    copied = args.output / "config/sessions" / source.parent.name / source.name
    private.update(source=source, copied=copied, before=before, temporary_before=temporary_before)
    copied.mkdir(parents=True, mode=0o700)
    for relative in before:
        target = copied / relative
        target.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
        # No hardlinks, raw journal rewriting, SQLite or derived read caches.
        result = subprocess.run(["cp", "--reflink=auto", "--sparse=always", "--", str(source / relative), str(target)],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                timeout=max(.1, args.deadline - time.monotonic()))
        if result.returncode:
            raise RuntimeError("canonical copy failed")
    source_after, temporary_after = canonical_inventory(source)
    clone_after, clone_temporary = canonical_inventory(copied)
    if (source_after != before or temporary_after != temporary_before
            or clone_after != before or clone_temporary):
        raise ValueError("canonical copy hash mismatch or concurrent source change")
    config = args.output / "config/configs/clone.toml"
    config.parent.mkdir(mode=0o700)
    contents = profile(args.module, 1)
    contents = contents.replace('path = ".proteus/events.jsonl"',
                                "path = " + json.dumps(str(args.output / "events.jsonl")))
    contents = contents.replace('enabled = ["read_file"]', 'enabled = []')
    # Explicitly disable probes in all four reference children, even if the
    # process host starts forwarding this variable in a later release.
    contents = contents.replace("command = " + json.dumps(str(args.module)),
                                "command = " + json.dumps(str(args.module)) + '\nenv = { PROTEUS_MEMORY_TRACE = "" }')
    if contents.count('env = { PROTEUS_MEMORY_TRACE = "" }') != 4:
        raise ValueError("expected four isolated reference components")
    if 'path = ".proteus/events.jsonl"' in contents or "hooks = []" not in contents:
        raise ValueError("profile isolation was not applied")
    config.write_text(contents)
    return source, copied, workspace, expected_id, before, config


def environment(args):
    env = os.environ.copy()
    for key, relative in (("HOME", "home"), ("PROTEUS_HOME", "runtime"),
                          ("PROTEUS_CONFIG_HOME", "config"), ("XDG_CONFIG_HOME", "xdg/config"),
                          ("XDG_DATA_HOME", "xdg/data"), ("XDG_CACHE_HOME", "xdg/cache")):
        path = args.output / relative
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
        env[key] = str(path)
    env.update(NO_PROXY="localhost,127.0.0.1,::1", no_proxy="localhost,127.0.0.1,::1")
    env.pop("PROTEUS_MEMORY_TRACE", None)
    if args.memory_trace:
        env["PROTEUS_MEMORY_TRACE"] = str(args.output / "memory-trace.jsonl")
    return env


def shutdown(origin):
    if origin is None:
        return False
    try:
        request = Request(local_origin(origin) + "/shutdown", data=b"{}",
                          headers={"Content-Type": "application/json"})
        with build_opener(ProxyHandler({})).open(request, timeout=3) as response:
            response.read(65536)
        return True
    except (OSError, ValueError):
        return False


def validate_memory_trace(path, cycles, tree):
    """Require real content-free probes from both measured backend identities."""
    if set(cycles) != {"first", "restart"}:
        raise ValueError("trace requires both measured cycles")
    owners = {}
    for cycle, result in cycles.items():
        identity = (result["backend_pid"], result["backend_start_ticks"])
        if identity not in tree.sampled_backend_identities:
            raise ValueError("trace backend was not actually sampled")
        if identity[0] in owners:
            raise ValueError("trace PID reuse is ambiguous")
        owners[identity[0]] = cycle
    found = {cycle: {"records": 0, "phases": set()} for cycle in cycles}
    with path.open() as stream:
        for line in stream:
            row = json.loads(line)
            if (not isinstance(row, dict) or type(row.get("pid")) is not int
                    or not isinstance(row.get("phase"), str) or not row["phase"]
                    or type(row.get("wall_time_ns")) is not int or row["wall_time_ns"] <= 0):
                raise ValueError("invalid memory trace record")
            if row["pid"] not in owners:
                raise ValueError("memory probe came from an unmeasured process")
            actual = found[owners[row["pid"]]]
            actual["records"] += 1
            actual["phases"].add(row["phase"])
    if not found["first"]["phases"].intersection({"journal.raw_records_loaded", "journal.stream_validated"}):
        raise ValueError("cold trace lacks a complete journal read")
    prepared_phases = {"prepared.into_display_view", "prepared.full_model_dropped",
                       "read_view.from_model", "read_view.page_selected"}
    if not found["restart"]["phases"].intersection(prepared_phases):
        raise ValueError("restart trace lacks prepared/read-view probes")
    return {"validated": True, "backend_identities": [
                {"cycle": cycle, "pid": result["backend_pid"], "start_ticks": result["backend_start_ticks"]}
                for cycle, result in cycles.items()],
            "cycles": {cycle: {"records": value["records"], "phases": sorted(value["phases"])}
                       for cycle, value in found.items()},
            "restart_raw_load_required": False}


def run(args):
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    report = {"status": "failed", "binary_sha256": digest(args.binary), "module_sha256": digest(args.module),
              "source_spec_sha256": digest(args.source_spec), "cycles": {},
              "content_artifacts": "private canonical clone and private runtime stderr only; no snapshot/stdout content artifacts",
              "http_operations": ["GET /health", "GET /events", "POST /shutdown"]}
    source = copied = before = metrics = tree = None
    processes, threads, handles = [], [], []
    private = {}
    origin = None
    try:
        source, copied, workspace, session_id, before, config = clone_source(args, private)
        report.update(source_sha256=encoded_digest(before), source_files=before,
                      session_id_sha256=opaque(session_id), source_copy_initially_identical=True,
                      copied_derived_cache=False)
        env = environment(args)
        tree = PrivateTree(os.getpid(), root_group="controller", classifier=classifier(args))
        metrics_args = SimpleNamespace(**vars(args))
        metrics_args.timeout = max(.1, args.deadline - time.monotonic())
        metrics = Metrics(tree, args.output, metrics_args)
        metrics.mark("before_launch")
        metrics.sample()  # Establish sampler baseline BEFORE Popen.
        for cycle in ("first", "restart"):
            origin = None
            metrics.mark(cycle + "_launch")
            metrics.sample()
            start = time.monotonic_ns()
            error_log = (args.output / (cycle + ".stderr.private.log")).open("w")
            handles.append(error_log)
            command = [str(args.binary), "--config", str(config), "--cwd", str(workspace),
                       "--resume-session", str(copied), "server", "http", "--host", "127.0.0.1",
                       "--port", "0", "--ready-stdout"]
            process = subprocess.Popen(command, cwd=workspace, env=env, stdout=subprocess.PIPE,
                                       stderr=error_log, text=True)
            processes.append(process)
            events = queue.Queue()
            worker = threading.Thread(target=observe, args=(process, copied, session_id, events,
                                      max(.1, args.deadline - time.monotonic())), daemon=True)
            worker.start()
            threads.append(worker)
            summary = None
            cycle_report = {}
            while summary is None:
                metrics.sample()
                if time.monotonic() > args.deadline:
                    raise TimeoutError("overall deadline reached")
                while not events.empty():
                    event = events.get_nowait()
                    if event["type"] == "error":
                        raise RuntimeError("HTTP reader failed")
                    if event["type"] == "http_bound":
                        origin = event["origin"]
                        cycle_report["http_bound_ms"] = (event["monotonic_ns"] - start) / 1e6
                    elif event["type"] == "snapshot_ready":
                        summary = event["summary"]
                        cycle_report["snapshot_ready_ms"] = (event["monotonic_ns"] - start) / 1e6
                if process.poll() is not None:
                    raise RuntimeError("backend exited before snapshot")
                if summary is None:
                    time.sleep(args.interval)
            metrics.mark(cycle + "_snapshot_ready")
            metrics.sample()
            cycle_report.update(backend_pid=process.pid, backend_start_ticks=tree.owned[process.pid])
            cycle_report["snapshot"] = summary
            report["cycles"][cycle] = cycle_report
            if cycle == "restart" and summary != report["cycles"]["first"]["snapshot"]:
                raise ValueError("cold/restart snapshot summaries differ")
            metrics.mark(cycle + "_resident_idle")
            metrics.pause(args.idle_seconds, [process])
            if args.memory_trace and not (args.output / "memory-trace.jsonl").is_file():
                raise RuntimeError("memory diagnostics trace absent; use a feature-enabled binary")
            metrics.mark(cycle + "_teardown")
            cycle_report["shutdown_http_ok"] = shutdown(origin)
            origin = None
            graceful_deadline = min(args.deadline, time.monotonic() + 3)
            while process.poll() is None and time.monotonic() < graceful_deadline:
                metrics.sample()
                time.sleep(args.interval)
            cycle_report["normal_exit_before_cleanup"] = process.poll() == 0
            cleanup([process], tree)
            worker.join(timeout=2)
            metrics.sample()
            if canonical_files(copied) != before:
                raise ValueError("resumed canonical clone changed")
        report["status"] = "passed"
    except BaseException as exc:
        # Exception strings/tracebacks may contain private paths or content.
        report["error_type"] = type(exc).__name__
    finally:
        shutdown(origin)
        if tree is not None:
            try:
                if metrics is not None:
                    metrics.mark("final_cleanup")
                cleanup(processes, tree)
            except Exception as exc:
                report.update(status="failed", cleanup_error_type=type(exc).__name__)
        for thread in threads:
            thread.join(timeout=2)
        for process in processes:
            if process.stdout:
                process.stdout.close()
        for handle in handles:
            handle.close()
        if args.memory_trace and tree is not None:
            try:
                report["memory_trace"] = validate_memory_trace(
                    args.output / "memory-trace.jsonl", report["cycles"], tree)
            except Exception as exc:
                report.update(status="failed", memory_trace={"validated": False,
                              "error_type": type(exc).__name__})
        if metrics is not None:
            metrics.close()
            report["measurement"] = save_partial(args.output, metrics, report.get("error_type"))
        source = source or private.get("source")
        copied = copied or private.get("copied")
        before = before or private.get("before")
        if source is not None and before is not None:
            report.setdefault("source_files", before)
            report.setdefault("source_sha256", encoded_digest(before))
            try:
                source_after, temporary_after = canonical_inventory(source)
                report["source_files_after"] = source_after
                report["source_canonical_unchanged"] = source_after == before
                temporary_before = private.get("temporary_before", {})
                report["skipped_source_temporary_files"] = {
                    "count": len(temporary_before),
                    "bytes": sum(value["bytes"] for value in temporary_before.values()),
                    "unchanged": temporary_after == temporary_before,
                    "copied": False}
                clone_after, clone_temporary = canonical_inventory(copied)
                report["clone_files_after"] = clone_after
                report["clone_canonical_unchanged"] = clone_after == before and not clone_temporary
                if (not report["source_canonical_unchanged"] or not report["clone_canonical_unchanged"]
                        or not report["skipped_source_temporary_files"]["unchanged"]):
                    report["status"] = "failed"
            except Exception as exc:
                report.update(status="failed", canonical_check_error_type=type(exc).__name__)
        report["no_executions_started"] = (report.get("clone_canonical_unchanged") is True
            and len(report["cycles"]) == 2
            and all(not cycle["snapshot"]["active_execution"] for cycle in report["cycles"].values()))
        report["no_snapshot_content_artifacts"] = not any(
            path.name.endswith((".snapshot.json", ".snapshot.json.gz", ".stdout.log"))
            for path in args.output.rglob("*") if path.is_file())
        if not report["no_executions_started"] or not report["no_snapshot_content_artifacts"]:
            report["status"] = "failed"
        write_json(args.output / "report.json", report)
    print(json.dumps({"status": report["status"], "source_sha256": report.get("source_sha256"),
                      "error_type": report.get("error_type")}), flush=True)
    return 0 if report["status"] == "passed" else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-spec", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--module", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--memory-trace", action="store_true")
    parser.add_argument("--idle-seconds", type=float, default=2)
    parser.add_argument("--interval", type=float, default=.05)
    parser.add_argument("--max-pss-mib", type=int, default=3072)
    parser.add_argument("--timeout", type=float, default=180)
    args = parser.parse_args()
    if not (.05 <= args.interval <= 5 and 0 < args.idle_seconds <= 60
            and 1 <= args.max_pss_mib <= 8192 and 1 <= args.timeout <= 600
            and all(math.isfinite(value) for value in (args.interval, args.idle_seconds, args.timeout))):
        parser.error("invalid sampling/deadline/memory bounds")
    try:
        for name in ("source_spec", "binary", "module"):
            path = getattr(args, name).resolve(strict=True)
            if not path.is_file() or (name != "source_spec" and not os.access(path, os.X_OK)):
                raise ValueError("invalid input")
            setattr(args, name, path)
        args.output = args.output.resolve()
        if args.output.exists():
            raise ValueError("output must be new")
        args.deadline = time.monotonic() + args.timeout
        previous_term = signal.getsignal(signal.SIGTERM)
        def interrupted(_signal, _frame):
            raise InterruptedError("termination requested")
        signal.signal(signal.SIGTERM, interrupted)
        try:
            return run(args)
        finally:
            signal.signal(signal.SIGTERM, previous_term)
    except Exception as exc:
        print(json.dumps({"status": "failed", "error_type": type(exc).__name__}), flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

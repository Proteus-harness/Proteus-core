"""Ten independent read-only HTTP backends, first launch and same-copy restart."""
import gzip
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time
from urllib.parse import quote
from urllib.request import urlopen, Request

from measurement import Metrics, Tree, classifier, cleanup, save_partial


def check_snapshot(snapshot, entry):
    if snapshot["session_id"] != entry["session_id"]:
        raise ValueError("HTTP snapshot resumed the wrong seeded session")
    transcript = snapshot["transcript"]
    before = (snapshot.get("history") or {}).get("before", 0)
    if before + len(transcript) != entry["rows"]:
        raise ValueError("HTTP history cursor + visible rows does not match seeded count")
    actual = [dict(message_id=message["message_id"], text=message["text"]) for message in transcript[-50:]]
    if actual != entry["tail"]:
        raise ValueError("HTTP snapshot tail changed seeded message IDs/text")
    return dict(session_id=snapshot["session_id"], visible_messages=len(transcript),
                history_before=before, total_messages=before + len(transcript), tail_matched=True)


def observe(process, config, entry, case, cycle, events, timeout):
    """One daemon reader per owned process. No full /history request is made."""
    peer = entry["peer"]
    try:
        origin = None
        with (case / f"{cycle}-peer-{peer}.stdout.log").open("w") as output:
            for line in process.stdout:
                output.write(line)
                output.flush()
                if line.startswith("{"):
                    ready = json.loads(line)
                    if ready.get("type") == "http_ready":
                        origin = ready["origin"]
                        break
            if origin is None:
                raise RuntimeError(f"Core exited before HTTP readiness; status={process.poll()}")
            events.put(dict(type="http_bound", peer=peer, origin=origin, monotonic_ns=time.monotonic_ns()))
            with urlopen(origin + "/health", timeout=min(timeout, 30)) as response:
                if json.load(response).get("ok") is not True:
                    raise ValueError("HTTP health did not confirm readiness")
            url = origin + "/events?session_dir=" + quote(entry["session_dir"], safe="")
            with urlopen(url, timeout=min(timeout, 30)) as response:
                for line in response:
                    if not line.startswith(b"data: "):
                        continue
                    record = json.loads(line[6:])
                    if record.get("type") == "response" and record.get("ok") is False:
                        raise ValueError(f"HTTP event stream failed: {record}")
                    event = record.get("event", {})
                    if event.get("type") == "session_snapshot":
                        summary = check_snapshot(event["snapshot"], entry)
                        ready_ns = time.monotonic_ns()
                        with gzip.open(case / f"{cycle}-peer-{peer}.snapshot.json.gz", "wb", compresslevel=1) as saved:
                            saved.write(line[6:])
                        events.put(dict(type="snapshot_ready", peer=peer, origin=origin,
                                        monotonic_ns=ready_ns, bytes=len(line) - 6, **summary))
                        # Baseline may return 50k rows while current returns 50.
                        # Do not retain the parsed baseline in the idle reader.
                        del record, event, summary, line
                        break
                else:
                    raise RuntimeError("HTTP event stream ended before its session snapshot")
            # Keep draining diagnostics; stdout must never block the child.
            for line in process.stdout:
                output.write(line)
                output.flush()
    except Exception as error:
        events.put(dict(type="error", peer=peer, monotonic_ns=time.monotonic_ns(), error=repr(error)))


def run(args, case, manifest):
    # This supervisor has no pre-existing children. Its own memory/CPU is grouped
    # as controller and excluded from reported backend+module subject metrics.
    tree = Tree(os.getpid(), root_group="controller", classifier=classifier(args))
    metrics = Metrics(tree, case, args)
    all_processes, all_threads, handles = [], [], []
    readiness = {}
    failure = None
    try:
        for cycle in ("first", "restart"):
            metrics.mark(f"{cycle}_launch", cache="warm OS cache; restart retains derived files" if cycle == "restart" else "fresh copied fixture; OS cache uncontrolled")
            started = time.monotonic_ns()
            events = queue.Queue()
            processes = []
            peer_processes = {}
            spawn_times = {}
            for config, entry in manifest["entries"].items():
                command = [str(args.binary), "--config", config, "--cwd", manifest["workspace"],
                           "--resume-session", entry["session_dir"], "server", "http",
                           "--host", "127.0.0.1", "--port", "0", "--ready-stdout"]
                error = (case / f"{cycle}-peer-{entry['peer']}.stderr.log").open("w")
                handles.append(error)
                spawn_times[entry["peer"]] = time.monotonic_ns()
                process = subprocess.Popen(command, cwd=manifest["workspace"], env=args.env,
                                           stdout=subprocess.PIPE, stderr=error, text=True)
                processes.append(process)
                peer_processes[entry["peer"]] = process
                all_processes.append(process)
                worker = threading.Thread(target=observe,
                    args=(process, config, entry, case, cycle, events, args.timeout), daemon=True)
                worker.start()
                all_threads.append(worker)
            bound, snapshots = {}, {}
            while len(snapshots) < args.peers:
                metrics.sample()
                while True:
                    try:
                        event = events.get_nowait()
                    except queue.Empty:
                        break
                    metrics.log.write(dict(cycle=cycle, **event))
                    if event["type"] == "error":
                        raise RuntimeError(event["error"])
                    if event["type"] == "http_bound":
                        bound[event["peer"]] = event
                    elif event["type"] == "snapshot_ready":
                        snapshots[event["peer"]] = event
                if any(process.poll() is not None for process in processes):
                    raise RuntimeError("Core exited before all HTTP session snapshots were ready")
                if len(snapshots) < args.peers:
                    time.sleep(args.interval)
            readiness[cycle] = dict(
                all_http_bound_ms=(max(event["monotonic_ns"] for event in bound.values()) - started) / 1e6,
                all_snapshots_ready_ms=(max(event["monotonic_ns"] for event in snapshots.values()) - started) / 1e6,
                peers=[dict(peer=peer, http_bound_ms=(bound[peer]["monotonic_ns"] - spawn_times[peer]) / 1e6,
                            snapshot_ready_ms=(snapshots[peer]["monotonic_ns"] - spawn_times[peer]) / 1e6,
                            pid=peer_processes[peer].pid, start_ticks=tree.owned[peer_processes[peer].pid],
                            snapshot=snapshots[peer]) for peer in sorted(snapshots)])
            metrics.mark(f"{cycle}_resident_idle", seconds=args.idle_seconds)
            metrics.pause(args.idle_seconds, processes)
            metrics.mark(f"{cycle}_teardown")
            for event in bound.values():
                try:
                    request = Request(event["origin"] + "/shutdown", data=b"{}", headers={"Content-Type": "application/json"})
                    with urlopen(request, timeout=5) as response:
                        response.read()
                except (OSError, ValueError):
                    pass
            cleanup(processes, tree)
            metrics.sample()
        metrics.mark("done")
    except Exception as error:
        failure = repr(error)
        raise
    finally:
        try:
            cleanup(all_processes, tree)
        finally:
            for thread in all_threads:
                thread.join(timeout=2)
            for process in all_processes:
                if process.stdout:
                    process.stdout.close()
            for handle in handles:
                handle.close()
            metrics.close()
            result = save_partial(case, metrics, failure)
            result["readiness"] = readiness
            (case / "measurement.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
    return result

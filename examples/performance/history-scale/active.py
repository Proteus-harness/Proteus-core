"""Real AgentControl spawn/wait/resume with one strict seeded role per peer."""
import json
import os
from pathlib import Path
import queue
import signal
import subprocess
import threading
import time

from fixtures import HERE, write_json
from launcher import MANIFEST_ENV
from measurement import Metrics, Tree, classifier, cleanup, save_partial


def abort_unobserved_group(process):
    """Popen created a fresh session; keep its child PID reserved until killed.

    Do not poll/wait between group signals: an unreaped child reserves its PID,
    so that process-group ID cannot be recycled while initialization fails.
    Normal initialized cleanup uses the sampler's PID/starttime identities.
    """
    try:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        time.sleep(0.1)
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=3)
    finally:
        if process.stdout:
            process.stdout.close()


def overlap(manifest, expected_peers, rounds):
    records = []
    for config in manifest["entries"]:
        path = Path(config).parent.parent / ".proteus/events.jsonl"
        with path.open() as stream:
            for line in stream:
                event = json.loads(line)
                if any(kind in event["event"] for kind in ("ModelRequestPrepared", "ModelResponseReceived")):
                    records.append(event)
    active, maximum, calls = set(), 0, 0
    for record in sorted(records, key=lambda event: event["timestamp_ms"]):
        identity = (record["session_id"], record["thread_id"])
        if "ModelRequestPrepared" in record["event"]:
            if identity in active:
                raise ValueError("overlapping fake requests within one sequential peer")
            active.add(identity)
            maximum = max(maximum, len(active))
            calls += 1
        elif "ModelResponseReceived" in record["event"]:
            if identity not in active:
                raise ValueError("fake response without request")
            active.remove(identity)
    if active or maximum != expected_peers or calls != expected_peers * rounds * 2:
        raise ValueError(f"fake model concurrency/count mismatch: maximum={maximum}, calls={calls}")
    return dict(maximum_in_flight=maximum, model_calls=calls)


def run(args, case, manifest):
    launcher = HERE / "launcher.py"
    if not os.access(launcher, os.X_OK):
        raise ValueError("benchmark launcher.py must be executable (tracked git mode 100755)")
    path = case / "input.json"
    write_json(path, dict(binary=str(launcher), config=manifest["parent_config"], workspace=manifest["workspace"],
                          peers=args.peers, rounds=args.rounds, idle_cap=args.peers,
                          peer_configs=list(manifest["entries"]), resident_idle_ms=int(args.idle_seconds * 1000),
                          role_timeout_ms=int(args.timeout * 1000)))
    env = dict(args.env, **{MANIFEST_ENV: str(case / "manifest.json")})
    messages = queue.Queue()
    events = []
    with (case / "driver.stderr.log").open("w") as error:
        process = subprocess.Popen([str(args.driver), str(path)], cwd=manifest["workspace"], env=env,
                                   stdout=subprocess.PIPE, stderr=error, text=True, start_new_session=True)
        try:
            tree = Tree(process.pid, root_group="controller", classifier=classifier(args))
            metrics = Metrics(tree, case, args)
        except BaseException:
            abort_unobserved_group(process)
            raise
        failure = None

        def read():
            try:
                with (case / "driver.events.jsonl").open("w") as output:
                    for line in process.stdout:
                        output.write(line)
                        output.flush()
                        event = json.loads(line)
                        event["observed_monotonic_ns"] = time.monotonic_ns()
                        messages.put(event)
            except Exception as exception:
                messages.put(dict(phase="reader_error", error=repr(exception)))

        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        metrics.mark("launch")
        try:
            while process.poll() is None:
                while True:
                    try:
                        event = messages.get_nowait()
                    except queue.Empty:
                        break
                    if event["phase"] == "reader_error":
                        raise RuntimeError(event["error"])
                    events.append(event)
                    phase = event["phase"]
                    if phase in ("round_start", "round_settled"):
                        phase += "_" + str(event["data"]["round"])
                    metrics.mark(phase, driver=event)
                metrics.sample()
                time.sleep(args.interval)
            reader.join(timeout=5)
            while not messages.empty():
                events.append(messages.get_nowait())
            if process.returncode:
                raise RuntimeError((case / "driver.stderr.log").read_text()[-6000:])
            if not any(event["phase"] == "done" for event in events):
                raise ValueError("driver completion event is missing")
            launched = [json.loads(line) for line in (case / "launcher.jsonl").read_text().splitlines()]
            if len(launched) != args.peers:
                raise ValueError("expected exactly one process launch per seeded role; resumed rounds must reuse it")
            model_overlap = overlap(manifest, args.peers, args.rounds)
            metrics.mark("done")
        except Exception as exception:
            failure = repr(exception)
            raise
        finally:
            try:
                cleanup([process], tree)
            finally:
                reader.join(timeout=5)
                process.stdout.close()
                metrics.close()
                result = save_partial(case, metrics, failure)
                result.update(events=events)
                if failure is None:
                    result["model_overlap"] = model_overlap
                write_json(case / "measurement.json", result)
    return result

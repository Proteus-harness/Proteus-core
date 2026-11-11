#!/usr/bin/env python3
"""Run two isolated Flash output-limit controls using the original workflow binary."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import threading

HERE = Path(__file__).resolve().parent
CORE = HERE.parents[2] if len(HERE.parents) > 2 else HERE
DATA = CORE / "target/performance/coding-harness"
CASES = ("journal-segments", "ledger-recovery")
MODEL = "opencode-go/deepseek-v4.1-flash"
CAP = 384000


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def rewrite(frame):
    """Change only the declared limit in workflow requests sent to the Core host."""
    paths = {
        "host.model.stream.start": ("request",),
        "host.model.complete": ("request",),
        "host.context.prepare": ("input", "request"),
    }
    path = paths.get(frame.get("method"))
    if path is None:
        return False
    request = frame.get("params", {}).get("params", {})
    for key in path:
        if not isinstance(request, dict):
            return False
        request = request.get(key)
    if not isinstance(request, dict):
        return False
    model = request.get("model", {})
    if model != {"provider": "pi_codex", "model": "deepseek-v4.1-flash"}:
        raise ValueError("Limit control received an unexpected model")
    limits = request["limits"]
    if limits["max_output_tokens"] != 16384:
        raise ValueError("Limit control expected the original 16384-token limit")
    limits["max_output_tokens"] = CAP
    return True


def proxy(binary):
    child = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=None)

    def forward_input():
        try:
            for line in sys.stdin.buffer:
                child.stdin.write(line)
                child.stdin.flush()
        except BrokenPipeError:
            pass
        finally:
            child.stdin.close()

    threading.Thread(target=forward_input, daemon=True).start()
    try:
        for line in child.stdout:
            frame = json.loads(line)
            if rewrite(frame):
                line = (json.dumps(frame, separators=(",", ":")) + "\n").encode()
                # Only cap evidence is written here. Request bodies and credentials
                # remain under the host's normal recording and cleanup rules.
                if os.access("/state", os.W_OK):
                    with Path("/state/cap-rewrites.jsonl").open("a") as trace:
                        trace.write(json.dumps({"method": frame["method"], "from": 16384,
                                                "to": CAP}) + "\n")
            sys.stdout.buffer.write(line)
            sys.stdout.buffer.flush()
    except BaseException:
        child.kill()
        child.wait()
        raise
    return child.wait()


def prepare_prefix(source, destination):
    bins = destination / "bin"
    bins.mkdir(parents=True)
    original = {}
    for path in sorted((source / "bin").iterdir()):
        if path.is_file():
            os.link(path, bins / path.name)
            original[path.name] = digest(path)
    worker = bins / "proteus-coding-workflow-module"
    worker.rename(worker.with_name(worker.name + ".original"))
    shutil.copy2(Path(__file__), bins / "limit_control.py")
    worker.write_text('''#!/usr/bin/python3
from pathlib import Path
import runpy
import sys
base = Path(__file__).resolve().parent
sys.argv = [str(base / "limit_control.py"), "--proxy", str(base / "proteus-coding-workflow-module.original")]
runpy.run_path(str(base / "limit_control.py"), run_name="__main__")
''')
    worker.chmod(0o755)
    (destination / "share").mkdir()
    for name in ("pi-runtime", "opencode-runtime"):
        (destination / "share" / name).symlink_to(source / "share" / name, target_is_directory=True)
    assert digest(bins / "proteus") == original["proteus"]
    assert digest(worker.with_name(worker.name + ".original")) == original[worker.name]
    assert all(digest(source / "bin" / name) == sha for name, sha in original.items())
    return original


def journal_requests(run):
    import adapters
    journal = adapters.load_module("limit_control_journal", HERE.parent / "history-scale/journal.py")
    paths = list((run / "runtime/sessions").rglob("journal.jsonl"))
    if len(paths) != 1:
        return []
    return [payload["request"] for row, payload in journal.records(paths[0].parent)
            if row["kind"] == "model_request_recorded"]


def initial_fields(request):
    # These are the identities that fresh independent sessions must change.
    # Tool schemas and arbitrary nested application fields remain exact.
    clean = json.loads(json.dumps(request))
    clean["limits"].pop("max_output_tokens")
    clean.get("cache", {}).pop("routing_key", None)
    for key in ("session_id", "thread_id", "turn_id"):
        clean.get("client_metadata", {}).pop(key, None)
    for message in clean["messages"]:
        message.pop("id", None)
        for part in message["parts"]:
            part.pop("part_id", None)
    return clean


def run_case(case, root, prefix):
    run = root / ("flash_" + case + "_1_proteus-codex-cap384000")
    case_dir = DATA / "long-cases" / case
    original = DATA / "long-flash-01" / ("flash_" + case + "_1_proteus-codex")
    command = [sys.executable, "-B", str(HERE / "runner.py"), "--agent", "proteus",
               "--binary", str(prefix / "bin/proteus"), "--base-config", str(CORE / "configs/codex.config.toml"),
               "--case-dir", str(case_dir), "--output", str(run), "--model", MODEL,
               "--reasoning", "high", "--timeout", "1200"]
    with (root / (case + ".runner.log")).open("w") as log:
        subprocess.run(command, cwd=CORE, stdout=log, stderr=subprocess.STDOUT)
    if not (run / "result.json").exists():
        raise RuntimeError("Control runner did not record a result for " + case)
    subprocess.run([sys.executable, "-B", str(HERE / "corpus.py"), "grade", str(case_dir),
                    "--workspace", str(run / "workspace"), "--output", str(run / "grading.json")],
                   cwd=CORE, stdout=subprocess.DEVNULL)
    return evidence_case(case, run, original)


def evidence_case(case, run, original):
    result = json.loads((run / "result.json").read_text())
    grade = json.loads((run / "grading.json").read_text())
    baseline = json.loads((original / "result.json").read_text())
    requests = journal_requests(run)
    previous = journal_requests(original)
    caps = [request["limits"]["max_output_tokens"] for request in requests]
    evidence = {
        "case": case, "source": str(run), "baseline": str(original),
        "run_path": str(run.relative_to(DATA)), "baseline_path": str(original.relative_to(DATA)),
        "output_cap": CAP, "observed_request_caps": caps,
        "baseline_initial_cap": previous[0]["limits"]["max_output_tokens"] if previous else None,
        "first_tools_sha256": hashlib.sha256(json.dumps(requests[0]["tools"], sort_keys=True).encode()).hexdigest() if requests else None,
        "same_binary": result["binary_sha256"] == baseline["binary_sha256"],
        "same_prompt": result["prompt_sha256"] == baseline["prompt_sha256"],
        "same_fixture": result["fixture_files_sha256"] == baseline["fixture_files_sha256"],
        "same_model_effort": (result["model"], result["reasoning"]) == (baseline["model"], baseline["reasoning"]),
        "same_initial_request_except_cap_and_ids": bool(requests and previous and initial_fields(requests[0]) == initial_fields(previous[0])),
        "pipeline_ok": result["pipeline_ok"], "grade": grade["passed"],
        "pass": result["pipeline_ok"] and grade["passed"], "returncode": result["returncode"],
        "seconds": result["wall_seconds"], "usage": result["metrics"].get("provider_usage"),
        "observed_usage": result["metrics"].get("partial_reported_usage_sums"),
        "usage_complete": all((result["metrics"].get("provider_usage") or {}).get(key) is not None for key in ("input_tokens", "output_tokens")),
        "model_requests": result["metrics"].get("model_requests"),
        "tools": result["metrics"].get("all_tool_calls"),
        "settlements": result["metrics"].get("settlements"),
    }
    evidence["control_verified"] = bool(caps and set(caps) == {CAP} and all(evidence[key] for key in (
        "same_binary", "same_prompt", "same_fixture", "same_model_effort", "same_initial_request_except_cap_and_ids")))
    (run / "control-evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print(case, "PASS" if evidence["pass"] else "FAIL", "control", evidence["control_verified"], flush=True)
    return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--proxy", type=Path)
    parser.add_argument("--output", type=Path, default=DATA / "long-cap-control")
    parser.add_argument("--selfcheck", action="store_true")
    parser.add_argument("--audit-output", type=Path, help="rebuild evidence for existing controls without generation")
    args = parser.parse_args()
    if args.proxy:
        return proxy(args.proxy)
    if args.selfcheck:
        model = {"provider": "pi_codex", "model": "deepseek-v4.1-flash"}
        frame = {"method": "host.model.stream.start", "id": "m:1:1", "params": {
            "invocation_id": "h:1:1", "params": {"request": {"model": model, "limits": {"max_input_tokens": 1000000, "max_output_tokens": 16384}}}}}
        before = json.loads(json.dumps(frame))
        assert rewrite(frame)
        before["params"]["params"]["request"]["limits"]["max_output_tokens"] = CAP
        assert frame == before
        assert not rewrite({"method": "host.tools.execute", "id": "other"})
        print("Output-cap-only rewrite self-check passed")
        return 0
    if args.audit_output:
        output = args.audit_output.resolve()
        results = []
        for case in CASES:
            run = output / ("flash_" + case + "_1_proteus-codex-cap384000")
            original = DATA / "long-flash-01" / ("flash_" + case + "_1_proteus-codex")
            results.append(evidence_case(case, run, original))
        summary_path = output / "summary.json"
        summary = json.loads(summary_path.read_text()) if summary_path.exists() else {}
        summary["cases"] = results
        summary_path.write_text(json.dumps(summary, indent=2) + "\n")
        return 0 if all(row["control_verified"] for row in results) else 1
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    source = CORE.parent / "Proteus-desktop/build/Proteus"
    prefix = output / "prefix"
    original = prepare_prefix(source, prefix)
    with ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(lambda case: run_case(case, output, prefix), CASES))
    source_unchanged = all(digest(source / "bin" / name) == sha for name, sha in original.items())
    summary = {"experiment": "Two fresh output-cap controls; stochastic trajectories may differ.",
               "cases": results, "published_bin_hashes": original,
               "published_bins_unchanged": source_unchanged}
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    return 0 if source_unchanged and all(result["control_verified"] for result in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())

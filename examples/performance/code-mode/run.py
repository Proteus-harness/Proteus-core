#!/usr/bin/env python3
"""Compare direct calls and CodeMode using the same live model and scored API tasks."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import signal
import shutil
import subprocess
import sys
import time

from fixtures import create_case, score_case
from measurements import measure, write_markdown

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
API_TOOLS = [f"lab__{name}" for name in ["list_issues", "get_issue", "update_queue", "submit_report"]]


def sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def revision(path):
    return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=path, text=True).strip()


def atomic_json(value, path):
    temporary = path.with_suffix(".pending")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    temporary.replace(path)


def config_for(mode, state, workspace, run_root, base, model, reasoning, timeout):
    instruction = (
        "Use code_mode to orchestrate this API task. Write your own JavaScript; tools.call takes "
        "exactly (name, args) and returns a ToolResult. Check .ok; JSON.parse(result.output) "
        "contains the structured API response. Await every operation and print only a short summary."
        if mode == "code-mode" else
        "Call the available API tools directly. You may batch independent tool calls in one "
        "response and use parallel tool calling. Check all tool results and complete every required operation.")
    return {
        "include": [str(base)], "profile": {"name": f"code-mode-eval-{mode}"},
        "providers": {"chatgpt": {"model": model, "reasoning": {"effort": reasoning, "summary": True}}},
        "instructions": [
            {"kind": "System", "file": str(ROOT / "configs/prompts/codex-default.md"), "priority": 100},
            {"kind": "System", "text": instruction, "priority": 110}],
        "agent_control": {"surface": "none", "roles": []},
        "modules": {"hooks": []},
        "module_config": {
            "context": {"codex_context": {"providers": ["environment"]}},
            "workflow": {"coding.codex_loop": {"stream_max_retries": 0}},
            "policy": {"codex_policy": {"allow": API_TOOLS + ["code_mode"], "allow_sandboxed": [], "ask_before": []}}},
        "tools": {"enabled": ["code_mode"] if mode == "code-mode" else [], "mcp_servers": [{
            "name": "lab", "command": sys.executable,
            "args": ["-B", str(HERE / "server.py"), "--state-dir", str(state)],
            "cwd": str(workspace), "protocol_version": "2025-11-25", "safety": "RunsCommands",
            "supports_parallel_tool_calls": False, "timeout_ms": 30_000,
            "max_response_bytes": 100_000}]},
        "runtime": {"model_timeout_ms": 120_000, "workflow_timeout_ms": timeout * 1000},
        "event_log": {"path": str(run_root / "events.jsonl")}}


def command_with_environment(arguments):
    environment = Path.home() / ".config/Proteus-agent/environment"
    if not environment.is_file():
        return arguments
    # Match the portable launcher's configured environment; never print values.
    return ["bash", "--noprofile", "--norc", "-c",
            'set -a; . "$1"; set +a; shift; exec "$@"',
            "codemode-eval", str(environment), *arguments]


def execute_run(case, mode, number, output, args):
    identity = f"{number:02d}-{case}-{mode}"
    run_root = output / identity
    run_root.mkdir()
    state = run_root / "state"
    task = create_case(case, state)
    workspace = output / "workspace"
    workspace.mkdir(exist_ok=True)
    config = config_for(mode, state, workspace, run_root, args.base_config,
                        args.model, args.reasoning, args.timeout)
    config_path = run_root / "config.json"
    atomic_json(config, config_path)
    (run_root / "prompt.txt").write_text(task["prompt"])
    env = os.environ.copy()
    env["PATH"] = str(args.binary.parent) + os.pathsep + env.get("PATH", "")
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    arguments = [str(args.binary), "--config", str(config_path), "--cwd", str(workspace), task["prompt"]]
    print(f"RUN {identity}", flush=True)
    started = time.monotonic()
    timed_out = False
    with (run_root / "stdout.log").open("w") as stdout, (run_root / "stderr.log").open("w") as stderr:
        process = subprocess.Popen(command_with_environment(arguments), env=env,
                                   stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                   start_new_session=True)
        try:
            code = process.wait(timeout=args.timeout + 30)
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(process.pid, signal.SIGTERM)
            try:
                code = process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                code = process.wait()
    elapsed = time.monotonic() - started
    score = score_case(case, state)
    result = {"id": identity, "case": case, "mode": mode, "returncode": code,
              "timed_out": timed_out, "wall_seconds": elapsed, "score": score,
              "passed": False, "fixture_seed": task["seed"], "fixture_count": task["issue_count"],
              "fixture_sha256": task["fixture_sha256"], "prompt_version": task["prompt_version"],
              "prompt_sha256": task["prompt_sha256"], "task_passed": False,
              "cli_shutdown_ok": code == 0 and not timed_out,
              "business_checks_passed": all(value for key, value in score["exact_checks"].items()
                                            if key != "report_exact")}
    journals = sorted(run_root.glob("sessions/**/journal.jsonl"))
    if len(journals) == 1:
        try:
            metrics = measure(journals[0].parent)
            result["metrics"] = metrics
            settled = metrics["settlements"] == [{"status": "success", "error": None}]
            exercised = (metrics["root_mcp_tool_calls"] > 0 if mode == "direct"
                         else metrics["nested_mcp_tool_calls"] > 0)
            complete = metrics["model_requests"] > 0 and metrics["model_requests"] == metrics["model_responses"]
            model_matches = all(json.loads(model)["model"] == args.model for model in metrics["model_refs"])
            result["task_passed"] = score["passed"] and settled and complete and model_matches
            result["mode_exercised"] = exercised
            result["passed"] = result["cli_shutdown_ok"] and result["task_passed"] and exercised
            result["api_delivery"] = {"root_mcp_calls": metrics["root_mcp_tool_calls"],
                                      "script_nested_calls": metrics["nested_mcp_tool_calls"],
                                      "hybrid": mode == "code-mode" and metrics["root_mcp_tool_calls"] > 0}
        except (ValueError, KeyError, OSError) as error:
            result["error"] = f"journal measurement failed: {error}"
    else:
        result["error"] = f"expected one new journal, found {len(journals)}"
    if code and not result.get("error"):
        result["error"] = f"CLI exit {code}; see {run_root / 'stderr.log'}"
    atomic_json(result, run_root / "result.json")
    metrics = result.get("metrics", {})
    usage = metrics.get("provider_usage", {})
    print(f"{'PASS' if result['passed'] else 'FAIL'} {identity}: {elapsed:.2f}s, "
          f"model_requests={metrics.get('model_requests')}, input={usage.get('input_tokens')}, "
          f"output={usage.get('output_tokens')}, api_calls={score.get('api_calls')}", flush=True)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="new result directory")
    parser.add_argument("--binary", type=Path, default=ROOT.parent / "Proteus-desktop/build/Proteus/bin/proteus")
    parser.add_argument("--base-config", type=Path, default=ROOT / "configs/code-mode.config.toml")
    parser.add_argument("--model", default="gpt-5.6-luna")
    parser.add_argument("--reasoning", default="high")
    parser.add_argument("--timeout", type=int, default=300)
    parser.add_argument("--case", choices=["audit", "update"], action="append")
    parser.add_argument("--mode", choices=["direct", "code-mode"], action="append")
    args = parser.parse_args()
    args.output = args.output.expanduser().resolve()
    args.binary = args.binary.expanduser().resolve()
    args.base_config = args.base_config.expanduser().resolve()
    if not args.binary.is_file() or not args.base_config.is_file():
        parser.error("existing binary and base-config required; this runner never builds")
    if not 30 <= args.timeout <= 900:
        parser.error("timeout must be between 30 and 900 seconds")
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"format": 1, "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "model": args.model, "reasoning": args.reasoning,
              "revisions": {"core": revision(ROOT), "modules": revision(ROOT.parent / "Proteus-modules")},
              "binary": {"path": str(args.binary), "sha256": sha256(args.binary)},
              "runtime_modules_sha256": {name: sha256(args.binary.parent / name) for name in
                                         ["proteus-code-mode-module", "proteus-model-pack-module"]
                                         if (args.binary.parent / name).is_file()},
              "benchmark_sources_sha256": {name: sha256(HERE / name) for name in
                                            ["run.py", "measurements.py", "fixtures.py", "server.py"]},
              "base_config": str(args.base_config), "runs": []}
    atomic_json(report, args.output / "report.json")
    snapshot = args.output / "benchmark_sources"
    snapshot.mkdir()
    for name in report["benchmark_sources_sha256"]:
        shutil.copy2(HERE / name, snapshot / name)
    cases = args.case or ["audit", "update"]
    for case_index, case in enumerate(cases):
        modes = args.mode or (["direct", "code-mode"] if case_index % 2 == 0 else ["code-mode", "direct"])
        for mode in modes:
            result = execute_run(case, mode, len(report["runs"]) + 1, args.output, args)
            previous = next((run for run in report["runs"] if run["case"] == case), None)
            if previous and previous["fixture_sha256"] != result["fixture_sha256"]:
                result["passed"] = False
                result["error"] = "fixture changed between modes"
            report["runs"].append(result)
            atomic_json(report, args.output / "report.json")
            write_markdown(report, args.output / "report.md")
    print(f"REPORT {args.output / 'report.md'}", flush=True)
    return 0 if all(run["passed"] for run in report["runs"]) else 1


if __name__ == "__main__":
    raise SystemExit(main())

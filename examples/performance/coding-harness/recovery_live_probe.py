#!/usr/bin/env python3
"""Explicitly opt-in, one bounded real DeepSeek run with one injected read cutoff."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil

import open_code_native
import recovery_probe
import runner

HERE = Path(__file__).resolve().parent
CORE = HERE.parents[2]
PROMPT = """This is a tiny task in a fresh disposable workspace. Use exec_command exactly once to run:
printf 'committed-live-effect\\n' >> /workspace/append.log; cat /workspace/append.log
This append is non-idempotent: after a successful tool result, do not execute it again and do not call other tools.
When the tool has succeeded, respond with exactly RECOVERY_LIVE_OK. No additional work is needed."""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--live", action="store_true", help="required explicit opt-in to paid native provider request")
    parser.add_argument("--portable", type=Path, default=CORE.parent / "Proteus-desktop/build/Proteus")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not args.live:
        parser.error("No requests made: --live is required")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    command = recovery_probe.prepare("codex", output, args.portable.resolve(), "recovered", True)
    command.remove("--unshare-net")
    command[-1] = PROMPT
    runtime = output / "runtime"
    executed_preload = runtime / "executed-recovery_live_preload.mjs"
    shutil.copyfile(HERE / "recovery_live_preload.mjs", executed_preload)
    command[command.index("/runtime/recovery_preload.mjs") - 1] = str(executed_preload)
    preload_hash = hashlib.sha256(executed_preload.read_bytes()).hexdigest()
    (output / "preload-provenance.json").write_text(json.dumps({
        "source": "runtime/executed-recovery_live_preload.mjs", "executed_preload_sha256": preload_hash,
        "provenance": "Exact source copied before launch and bound read-only into the component sandbox",
        "injected_bytes_content_saved": False,
    }, indent=2) + "\n")
    config_path = runtime / "configs/eval.json"
    config = json.loads(config_path.read_text())
    config["runtime"] = {"workflow_timeout_ms": 220000, "model_timeout_ms": 180000}
    config_path.write_text(json.dumps(config, indent=2) + "\n")
    # Only the Go key is staged privately. Never serialize its value in evidence.
    go_auth = open_code_native.fresh_go_export()
    secret = go_auth[0]["value"]["key"].encode()
    auth = runtime / "home/.pi/agent/auth.json"
    auth.unlink()  # Replace the offline probe's synthetic credential.
    open_code_native.write_private_json(auth, {
        "opencode-go": {"type": "api_key", "key": go_auth[0]["value"]["key"]},
    })
    environment = {name: value for name, value in os.environ.items() if name.lower() in
                   {"lang", "lc_all", "https_proxy", "http_proxy", "all_proxy", "no_proxy"}}
    environment["PATH"] = "/usr/bin:/bin"
    (output / "prompt.txt").write_text(PROMPT)
    try:
        status = runner.execute(command, output, 230, environment)
    finally:
        runner.remove_auth(runtime)
    paths = list((runtime / "sessions").rglob("journal.jsonl"))
    events = recovery_probe.records(paths[0].parent) if len(paths) == 1 else []
    calls = [p["call"] for h, p in events if h["kind"] == "tool_call_recorded" and p["phase"]["phase"] == "requested"]
    results = [p["result"] for h, p in events if h["kind"] == "tool_result_recorded"]
    effects = [p["result"] for h, p in events if h["kind"] == "tool_effect_recorded"]
    settlements = [p for h, p in events if h["kind"] == "turn_settled"]
    turns = {h["turn_id"] for h, _ in events if h["kind"] == "turn_opened"}
    transport = [json.loads(line) for line in (runtime / "live-transport.jsonl").read_text().splitlines()] if (runtime / "live-transport.jsonl").exists() else []
    requests = [item for item in transport if item["event"] == "request"]
    faults = [item for item in transport if item["event"] == "injected_transport_cutoff"]
    completed = [item for item in transport if item["event"] in {"real_stream_completed", "real_sse_done"}]
    completed_attempts = {item["attempt"] for item in completed}
    credential_leak = any(secret in path.read_bytes() for path in output.rglob("*") if path.is_file())
    disk = (output / "workspace/append.log").read_text() if (output / "workspace/append.log").exists() else ""
    final = (settlements[-1].get("output") or {}).get("text") if settlements else None
    call_ids = [item["id"] for item in calls]
    proof = {**status, "kind": "injected_transport_fault_on_real_provider",
             "executed_preload_sha256": preload_hash,
             "provider": "opencode-go", "model": "deepseek-v4.1-flash", "effort": "high",
             "sessions": len(paths), "turns": len(turns), "requests": requests, "faults": faults,
             "real_completed_streams": completed, "tool_call_ids": call_ids,
             "tool_effect_ids": [item["call_id"] for item in effects],
             "tool_result_ids": [item["call_id"] for item in results],
             "append_lines": disk.splitlines(), "final_text": final, "auth_removed": not auth.exists(),
             "credential_leak_detected": credential_leak}
    proof["effect_once"] = bool(disk == "committed-live-effect\n" and len(call_ids) == 1
                                and proof["tool_effect_ids"] == call_ids and proof["tool_result_ids"] == call_ids)
    proof["same_messages_tools_on_retry"] = bool(len(requests) == 3 and requests[2]["retry_preserves_messages_tools"])
    proof["native_selection_verified"] = bool(requests and all(item["model"] == "deepseek-v4.1-flash"
                                                              and item.get("reasoning_effort") == "high" for item in requests))
    proof["proof_ok"] = bool(status["returncode"] == 0 and not status["timed_out"] and proof["effect_once"]
                             and proof["same_messages_tools_on_retry"] and len(faults) == 1
                             and proof["native_selection_verified"]
                             and faults[0]["real_bytes_forwarded"] > 0 and faults[0]["committed_disk_effect"]
                             and len(paths) == len(turns) == 1 and completed_attempts == {1, 3}
                             and not credential_leak and not auth.exists()
                             and final == "RECOVERY_LIVE_OK")
    (output / "proof.json").write_text(json.dumps(proof, indent=2) + "\n")
    print(json.dumps({key: proof[key] for key in ["proof_ok", "effect_once", "same_messages_tools_on_retry", "returncode", "wall_seconds", "final_text"]}))
    return 0 if proof["proof_ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

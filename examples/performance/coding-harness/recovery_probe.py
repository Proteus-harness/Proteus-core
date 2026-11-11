#!/usr/bin/env python3
"""Prove resumed model sampling preserves committed shell effects, without a network."""
import argparse
import importlib.util
import json
from pathlib import Path
import shutil

import runner
import replay_audit

HERE = Path(__file__).resolve().parent
CORE = HERE.parents[2]
PROFILES = {"pi": "bash", "opencode": "shell", "codex": "exec_command"}


def records(session):
    spec = importlib.util.spec_from_file_location("recovery_journal", HERE.parent / "history-scale/journal.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return list(module.records(session))


def prepare(profile, output, portable, mode, native_provider):
    workspace = output / "workspace"
    runtime = output / "runtime"
    workspace.mkdir(parents=True)
    runtime.mkdir()
    (runtime / "home").mkdir()
    (runtime / "sessions").mkdir()
    (workspace / "counter.txt").write_text("0")
    configs = runtime / "configs"
    shutil.copytree(CORE / "configs", configs)
    modules = {"workflow": {"pi_loop": {"catalog_provider": "opencode-go"}},
               "hook": {"pi_compact": {"catalog_provider": "opencode-go"}}} if profile == "pi" else {}
    if profile == "codex":
        modules["tool"] = {"codex_patch": {"freeform": False}}
    modules.setdefault("policy", {})["codex_policy"] = {
        "allow": [PROFILES[profile]], "allow_sandboxed": [], "ask_before": [],
    }
    modules["policy"]["opencode_policy"] = {"rules": [{"permission": "*", "action": "allow"}]}
    modules["model"] = {"recovery-probe": {"tool": PROFILES[profile], "capture": "/state/requests.jsonl", "mode": mode}}
    config = {
        "include": ["/runtime/configs/" + profile + ".config.toml"],
        "active_provider": "recovery",
        "providers": {"recovery": {"provider": "recovery-probe", "model": "deepseek-v4.1-flash", "stream": True,
                                    "reasoning": {"effort": "high", "summary": True}}},
        "module_config": modules,
        "components": {"recovery-model": {"command": "/usr/bin/python3", "args": ["-B", "/runtime/recovery_model.py"],
                                           "exports": {"model": {"recovery-probe": {}}}}},
        "agent_control": {"surface": "none", "roles": []},
        "runtime": {"model_timeout_ms": 60000, "workflow_timeout_ms": 240000},
        "event_log": {"path": "/state/events.jsonl"},
    }
    (configs / "eval.json").write_text(json.dumps(config, indent=2) + "\n")
    if native_provider:
        config["providers"]["recovery"]["provider"] = "pi_codex"
        config["components"].pop("recovery-model")
        config["components"]["pi-model" if profile == "pi" else "recovery-native-model"] = {
            "command": "proteus-pi-module", "env_allowlist": ["HOME", "PI_CODING_AGENT_DIR", "PATH", "NODE_OPTIONS"],
            "env": {"NODE_OPTIONS": "--import /runtime/recovery_preload.mjs"},
            "exports": {"model": {"pi_codex": {}}},
        }
        config["module_config"]["model"] = {"pi_codex": {"implementation": "opencode-go"}}
        auth = runtime / "home/.pi/agent/auth.json"
        auth.parent.mkdir(parents=True)
        auth.write_text(json.dumps({"opencode-go": {"type": "api_key", "key": "synthetic-offline-key"}}))
        (configs / "eval.json").write_text(json.dumps(config, indent=2) + "\n")
    mounts = [(portable / "bin", "/runtime/bin"), (configs, "/runtime/configs"),
              (runtime / "sessions", "/runtime/sessions", True),
              (HERE / "recovery_model.py", "/runtime/recovery_model.py"),
              (CORE / "examples/modules/component_runtime.py", "/runtime/component_runtime.py")]
    if profile == "pi" or native_provider:
        mounts.append((portable / "share/pi-runtime", "/runtime/share/pi-runtime"))
    if profile == "opencode":
        mounts.append((portable / "share/opencode-runtime", "/runtime/share/opencode-runtime"))
    if native_provider:
        mounts.append((HERE / "recovery_preload.mjs", "/runtime/recovery_preload.mjs"))
    command = runner.sandbox_command([
        "/runtime/bin/proteus", "--config", "/runtime/configs/eval.json", "--cwd", "/workspace",
        "Perform the scripted recovery proof and report the result.",
    ], mounts, workspace, runtime)
    command.insert(1, "--unshare-net")
    return command


def trial(profile, root, portable, mode, native_provider):
    output = root / (profile + "-" + mode)
    output.mkdir()
    command = prepare(profile, output, portable, mode, native_provider)
    status = runner.execute(command, output, 250, {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"})
    runner.remove_auth(output / "runtime")
    journals = list((output / "runtime/sessions").rglob("journal.jsonl"))
    if len(journals) != 1:
        raise ValueError(f"Expected one journal for {profile}, inspect {output / 'stderr.log'}")
    events = records(journals[0].parent)
    requests = [payload for header, payload in events if header["kind"] == "model_request_recorded"]
    outcomes = [payload for header, payload in events if header["kind"] == "model_response_recorded"]
    tool_calls = [payload["call"] for header, payload in events if header["kind"] == "tool_call_recorded"
                  and payload["phase"]["phase"] == "requested"]
    tool_results = [payload["result"] for header, payload in events if header["kind"] == "tool_result_recorded"]
    tool_effects = [payload["result"] for header, payload in events if header["kind"] == "tool_effect_recorded"]
    turns = {header["turn_id"] for header, _ in events if header["kind"] == "turn_opened"}
    settlements = [payload for header, payload in events if header["kind"] == "turn_settled"]
    captures = [json.loads(line) for line in (output / "runtime/requests.jsonl").read_text().splitlines()]
    result = {**status, "profile": profile, "mode": mode, "actual_pi_provider": native_provider, "model_requests": len(requests),
              "model_outcomes": len(outcomes), "tool_call_ids": [call["id"] for call in tool_calls],
              "tool_result_ids": [value["call_id"] for value in tool_results],
              "tool_effect_ids": [value["call_id"] for value in tool_effects],
              "committed_counter": (output / "workspace/counter.txt").read_text(),
              "partial_tool_executed": (output / "workspace/partial-marker.txt").exists(),
              "turns": len(turns), "settlements": settlements,
              "sampling_history_preserved": len(captures) > 2 and captures[1]["request"] == captures[2]["request"],
              "sessions": len(journals)}
    result["effect_once"] = (result["committed_counter"] == "1" and result["tool_call_ids"] == ["recovery-append-once"]
                             and result["tool_result_ids"] == ["recovery-append-once"]
                             and result["tool_effect_ids"] == ["recovery-append-once"] and not result["partial_tool_executed"])
    expected_success = mode == "recovered"
    result["final_text"] = (settlements[-1].get("output") or {}).get("text") if settlements else None
    expected_requests = 2 if mode == "terminal" else 3 if expected_success else {"pi": 5, "opencode": 12, "codex": 7}[profile]
    retry_count_ok = len(requests) == expected_requests
    result["proof_ok"] = bool(result["effect_once"] and result["turns"] == 1 and len(settlements) == 1 and retry_count_ok
                              and (status["returncode"] == 0) == expected_success
                              and (not expected_success or result["sampling_history_preserved"] and result["final_text"] == "RECOVERY_OK"))
    report = {**status, "agent": "proteus", "auth_removed": True, "command": command}
    (output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    # Synthetic model has no credentials. Keep replay's network/filesystem proof
    # while skipping scans of unrelated real provider credentials.
    replay_audit.credential_bytes = lambda: []
    replay = replay_audit.audit(output, output / "replay")
    result["replay_ok"] = replay["audit_ok"]
    result["proof_ok"] = result["proof_ok"] and replay["audit_ok"]
    (output / "proof.json").write_text(json.dumps(result, indent=2) + "\n")
    print(profile, mode, "PASS" if result["proof_ok"] else "FAIL", len(requests), "model requests", flush=True)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--portable", type=Path, default=CORE.parent / "Proteus-desktop/build/Proteus")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--profiles", choices=PROFILES, nargs="+", default=list(PROFILES))
    parser.add_argument("--mode", choices=("recovered", "terminal", "exhausted"), default="recovered")
    parser.add_argument("--native-provider", action="store_true", help="exercise actual Pi ModelRuntime with an offline SSE disconnect")
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    if args.native_provider and args.mode != "recovered":
        parser.error("native provider probe supports recovered mode")
    results = [trial(profile, args.output, args.portable.resolve(), args.mode, args.native_provider) for profile in args.profiles]
    (args.output / "summary.json").write_text(json.dumps(results, indent=2) + "\n")
    return 0 if all(result["proof_ok"] for result in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())

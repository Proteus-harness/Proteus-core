"""Typed fixture preparation and isolated per-run copies; never production data."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from journal import digest

HERE = Path(__file__).resolve().parent


def write_json(path, value):
    Path(path).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def prepare_fixtures(args):
    root = args.fixtures
    path = root / "fixtures.json"
    spec = dict(version=1, messages=args.messages, peers=args.peers, text_repeats=args.text_repeats,
                payload_kib=args.payload_kib, model_history_rows=args.model_history_rows)
    if path.is_file():
        manifest = json.loads(path.read_text())
        # Previously generated uncompacted fixtures had no optional tail field.
        # An omitted field means precisely the unchanged full-history scenario.
        existing_spec = dict(manifest["spec"])
        existing_spec.setdefault("model_history_rows", None)
        if existing_spec != spec:
            raise ValueError("existing fixture specification differs; choose another --fixtures directory")
        if Path(manifest["workspace"]).resolve() != root / "workspace" or digest(root / "workspace/payload.txt") != manifest["payload_sha256"]:
            raise ValueError("shared synthetic workspace/payload changed")
        for item in manifest["sessions"]:
            for relative, sha in item["canonical_files"].items():
                if digest(Path(item["session_dir"]) / relative) != sha:
                    raise ValueError("immutable source fixture changed")
        return manifest
    root.mkdir(parents=True, exist_ok=False)
    workspace = root / "workspace"
    workspace.mkdir()
    payload = ("SYNTHETIC_PAYLOAD_" + "0123456789abcdef" * 1024 * args.payload_kib)[:args.payload_kib * 1024]
    (workspace / "payload.txt").write_text(payload)
    (workspace / "AGENTS.md").write_text("Synthetic local resource benchmark.\n")
    sessions = []
    for rows in args.messages:
        for peer in range(args.peers):
            config_root = root / f"messages-{rows}" / f"peer-{peer}" / "config"
            config_root.mkdir(parents=True)
            command = [str(args.seeder), str(config_root), str(workspace), str(rows), str(args.text_repeats)]
            if args.model_history_rows is not None:
                command.append(str(args.model_history_rows))
            seeded = subprocess.run(command,
                                    check=True, capture_output=True, text=True, timeout=args.timeout,
                                    env=args.env)
            (config_root.parent / "seeder.stdout.json").write_text(seeded.stdout)
            (config_root.parent / "seeder.stderr.log").write_text(seeded.stderr)
            info = json.loads(seeded.stdout)
            session = Path(info["session_dir"]).resolve(strict=True)
            if (info["rows"] != rows or not session.is_relative_to(config_root / "sessions")
                    or (args.model_history_rows is not None and info.get("model_history_rows") != args.model_history_rows)):
                raise ValueError("typed seeder returned an unexpected session path/count")
            summary_path = config_root.parent / "seed.json"
            verify_command = [sys.executable, str(HERE / "verify.py"), "seed", str(session),
                              str(rows), str(args.text_repeats), str(summary_path)]
            if args.model_history_rows is not None:
                verify_command += ["--model-history-rows", str(args.model_history_rows)]
            subprocess.run(verify_command,
                           check=True, capture_output=True, text=True, timeout=args.timeout, env=args.env)
            canonical = {str(file.relative_to(session)): digest(file)
                         for file in sorted(session.rglob("*"))
                         if file.is_file() and (file.name == "journal.jsonl" or "blobs" in file.relative_to(session).parts)}
            sessions.append(dict(messages=rows, peer=peer, session_dir=str(session),
                                 relative_session=str(session.relative_to(config_root)),
                                 seed_summary=str(summary_path), canonical_files=canonical))
    manifest = dict(spec=spec, workspace=str(workspace), sessions=sessions,
                    seeder=dict(path=str(args.seeder), sha256=digest(args.seeder)),
                    payload_sha256=digest(workspace / "payload.txt"))
    write_json(path, manifest)
    return manifest


def profile(module, delay_ms):
    # Intentionally no compactor: its model-window clamp would shrink the corpus.
    # Fake accepts oversized context; this is not a real-provider context test.
    command = json.dumps(str(module))
    return f'''active_provider = "fake"
[providers.fake]
provider = "fake"
model = "fake-tool-model"
stream = true
[modules]
workflow = "coding.codex_loop"
context = "codex_context"
policy = "allow_all"
hooks = []
[components.reference-model]
command = {command}
[components.reference-model.exports.model.fake]
[components.reference-workflow]
command = {command}
[components.reference-workflow.exports.workflow."coding.codex_loop"]
[components.reference-context]
command = {command}
[components.reference-context.exports.context.codex_context]
[components.reference-capabilities]
command = {command}
[components.reference-capabilities.exports.policy.allow_all]
[components.reference-capabilities.exports.tool.file_tools]
[module_config.model.fake]
implementation = "fake"
stream_delay_ms = {delay_ms}
[module_config.context.codex_context]
providers = ["environment"]
max_context_bytes = 60000
[tools]
enabled = ["read_file"]
[agent_control]
surface = "none"
roles = []
[event_log]
path = ".proteus/events.jsonl"
'''


def prepare_case(args, fixtures, case, rows):
    if args.link_fixtures and args.mode != "resume-idle":
        raise ValueError("canonical hardlinks are forbidden for active AgentControl runs")
    case.mkdir(parents=True, exist_ok=False)
    entries = {}
    for source in fixtures["sessions"]:
        if source["messages"] != rows:
            continue
        root = case / "peers" / f"peer-{source['peer']}" / "config"
        configs = root / "configs"
        configs.mkdir(parents=True)
        session = root / source["relative_session"]
        def clone_file(source_file, target_file):
            relative = str(Path(source_file).relative_to(source["session_dir"]))
            if relative in source["canonical_files"]:
                if digest(source_file) != source["canonical_files"][relative]:
                    raise ValueError("source canonical fixture changed before cloning")
                if args.link_fixtures:
                    os.link(source_file, target_file)
                    return target_file
            # CoW when supported; byte-copy fallback preserves canonical source.
            subprocess.run(["cp", "--reflink=auto", "--sparse=always", "--preserve=mode,timestamps",
                            "--", source_file, target_file], check=True, capture_output=True)
            return target_file
        shutil.copytree(source["session_dir"], session, copy_function=clone_file)
        config = configs / "peer.toml"
        config.write_text(profile(args.module, args.delay_ms))
        seed = json.loads(Path(source["seed_summary"]).read_text())
        entries[str(config)] = dict(peer=source["peer"], session_dir=str(session),
                                   source_seed=str(source["seed_summary"]),
                                   canonical_files=source["canonical_files"],
                                   session_id=seed["session_id"], tail=seed["tail"], rows=rows,
                                   model_history_rows=seed.get("model_history_rows", rows))
    if len(entries) != args.peers or len({entry["session_id"] for entry in entries.values()}) != args.peers:
        raise ValueError("expected exactly one distinct seeded session per peer")
    parent = case / "parent-config" / "configs" / "parent.toml"
    parent.parent.mkdir(parents=True)
    parent.write_text(profile(args.module, args.delay_ms))
    manifest = dict(version=1, binary=str(args.binary), workspace=fixtures["workspace"],
                    entries=entries, launch_log=str(case / "launcher.jsonl"),
                    parent_config=str(parent), rounds=args.rounds,
                    model_history_rows=fixtures["spec"].get("model_history_rows"),
                    canonical_copy_mode="read-only-hardlinks" if args.link_fixtures else "reflink-or-copy")
    write_json(case / "manifest.json", manifest)
    return manifest

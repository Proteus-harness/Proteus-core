#!/usr/bin/env python3
"""Strict benchmark exec adapter: one allowlisted config resumes one seeded session."""
import json
import os
from pathlib import Path
import sys
import time

MANIFEST_ENV = "PROTEUS_HISTORY_BENCH_MANIFEST"


def plan(argv, manifest):
    if manifest.get("version") != 1:
        raise ValueError("unsupported benchmark manifest version")
    # AgentControl's exact current child invocation; reject optional arguments,
    # duplicate flags, custom subcommands and already-resumed invocations.
    if len(argv) != 7 or argv[0] != "--config" or argv[2] != "--cwd" or argv[4:] != ["--new-session", "server", "stdio"]:
        raise ValueError("expected exactly --config CONFIG --cwd WORKSPACE --new-session server stdio")
    config = Path(argv[1]).resolve(strict=True)
    workspace = Path(argv[3]).resolve(strict=True)
    if str(config) != argv[1] or str(workspace) != manifest["workspace"] or not workspace.is_dir():
        raise ValueError("config/workspace is not the canonical allowlisted path")
    entry = manifest["entries"].get(str(config))
    if entry is None:
        raise ValueError("config is absent from the benchmark allowlist")
    session = Path(entry["session_dir"]).resolve(strict=True)
    root = config.parent.parent
    if config.parent.name != "configs" or not session.is_relative_to(root / "sessions"):
        raise ValueError("seeded session must belong to the allowlisted config root")
    if not (session / "journal.jsonl").is_file():
        raise ValueError("seeded canonical journal is missing")
    binary = Path(manifest["binary"]).resolve(strict=True)
    if str(binary) != manifest["binary"] or not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError("actual Core binary is not an executable canonical file")
    return binary, [str(binary), "--config", str(config), "--cwd", str(workspace),
                    "--resume-session", str(session), "server", "stdio"]


def main():
    manifest = json.loads(Path(os.environ[MANIFEST_ENV]).read_text())
    binary, command = plan(sys.argv[1:], manifest)
    # One append-only evidence record; stdout remains the real Core protocol.
    record = json.dumps(dict(pid=os.getpid(), monotonic_ns=time.monotonic_ns(), argv=command)) + "\n"
    fd = os.open(manifest["launch_log"], os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
    try:
        os.write(fd, record.encode())
    finally:
        os.close(fd)
    os.execv(binary, command)


if __name__ == "__main__":
    main()

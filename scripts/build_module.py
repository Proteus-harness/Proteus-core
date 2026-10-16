#!/usr/bin/env python3
"""Build a module from Proteus-modules and print its actual executable path."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

from sources import resolve


def build(profile="dev", target_dir=None, env=None):
    source = resolve("modules")
    command = ["cargo", "build", "--locked", "--manifest-path", str(source / "Cargo.toml"),
               "-p", "proteus-reference-module", "--bin", "proteus-reference-module",
               "--profile", profile, "--message-format=json-render-diagnostics"]
    if target_dir:
        command += ["--target-dir", str(Path(target_dir).resolve())]
    child = subprocess.Popen(command, cwd=source, env=env, stdout=subprocess.PIPE, text=True)
    executable = None
    for line in child.stdout:
        message = json.loads(line)
        if message.get("reason") == "compiler-message":
            print(message["message"].get("rendered", ""), end="", file=sys.stderr)
        elif (message.get("reason") == "compiler-artifact" and message.get("executable")
              and message["target"]["name"] == "proteus-reference-module"):
            executable = message["executable"]
    status = child.wait()
    if status:
        raise SystemExit(status)
    if not executable:
        raise SystemExit("Module build did not emit an executable")
    return executable


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", default="dev")
    parser.add_argument("--target-dir")
    args = parser.parse_args()
    print(build(args.profile, args.target_dir))

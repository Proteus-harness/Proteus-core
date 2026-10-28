#!/usr/bin/env python3
"""Build a module from Proteus-modules and print its actual executable path."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

from sources import resolve


def build(profile="dev", target_dir=None, env=None):
    """Build the aggregate test fixture, never the production module delivery."""
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


def build_packages(profile="dev", target_dir=None, env=None, destination=None, reuse=False):
    source = resolve("modules")
    command = [sys.executable, str(source / "package.py"), "--profile", profile]
    command += ["--bundle", str(destination)] if destination else ["--print-artifacts"]
    if target_dir:
        command += ["--target-dir", str(Path(target_dir).resolve())]
    if reuse:
        command.append("--no-build")
    output = subprocess.check_output(command, cwd=source, env=env, text=True)
    return None if destination else json.loads(output)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", default="dev")
    parser.add_argument("--target-dir")
    parser.add_argument("--bundle", type=Path, help="deliver independent production modules into this runtime root")
    parser.add_argument("--no-build", action="store_true", help="reuse recorded production artifacts with --bundle")
    args = parser.parse_args()
    if args.bundle:
        build_packages(args.profile, args.target_dir, destination=args.bundle, reuse=args.no_build)
    else:
        print(build(args.profile, args.target_dir))

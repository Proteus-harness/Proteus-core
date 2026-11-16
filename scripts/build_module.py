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


def build_packages(profile="dev", target_dir=None, env=None, destination=None, reuse=False,
                   node_runtimes=False):
    """Resolve caller-owned output paths before changing the child working directory."""
    if node_runtimes and destination is None:
        raise ValueError("--node-runtimes requires --bundle")
    source = resolve("modules")
    bundle = Path(destination).resolve() if destination is not None else None
    command = [sys.executable, str(source / "package.py"), "--profile", profile]
    command += ["--bundle", str(bundle)] if bundle is not None else ["--print-artifacts"]
    if target_dir:
        command += ["--target-dir", str(Path(target_dir).resolve())]
    if reuse:
        command.append("--no-build")
    output = subprocess.check_output(command, cwd=source, env=env, text=True)
    if node_runtimes:
        for relative in ("tool/pi-tools/install.py", "workflow/opencode-runtime/install.py"):
            subprocess.run([sys.executable, str(source / relative), "--prefix", str(bundle)],
                           cwd=source, env=env, check=True)
    return None if bundle is not None else json.loads(output)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", default="dev")
    parser.add_argument("--target-dir")
    parser.add_argument("--bundle", type=Path, help="deliver independent production modules into this runtime root")
    parser.add_argument("--no-build", action="store_true", help="reuse recorded production artifacts with --bundle")
    parser.add_argument("--node-runtimes", action="store_true",
                        help="also install pinned Pi/OpenCode Node workers into --bundle")
    args = parser.parse_args()
    if args.node_runtimes and args.bundle is None:
        parser.error("--node-runtimes requires --bundle")
    if args.bundle:
        build_packages(args.profile, args.target_dir, destination=args.bundle, reuse=args.no_build,
                       node_runtimes=args.node_runtimes)
    else:
        print(build(args.profile, args.target_dir))

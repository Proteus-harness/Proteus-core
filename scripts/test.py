#!/usr/bin/env python3
"""Run harness tests with independently built process-module fixtures."""
import os
from pathlib import Path
import subprocess
import sys
import time

from build_module import build
from sources import ROOT, resolve


def test_env():
    env = os.environ.copy()
    env.setdefault("CARGO_BUILD_JOBS", "2")
    env.setdefault("RUST_TEST_THREADS", "4")
    env.setdefault("PYTHONDONTWRITEBYTECODE", "1")
    for key in ("NO_PROXY", "no_proxy"):
        values = [value for value in env.get(key, "").split(",") if value]
        env[key] = ",".join(dict.fromkeys([*values, "localhost", "127.0.0.1", "::1"]))
    return env


def selected_packages(args):
    packages = []
    for index, arg in enumerate(args):
        if arg in ("-p", "--package") and index + 1 < len(args):
            packages.append(args[index + 1])
        elif arg.startswith("--package="):
            packages.append(arg.split("=", 1)[1])
        elif arg.startswith("-p") and len(arg) > 2:
            packages.append(arg[2:])
    return packages


def main(args):
    if not args or args == ["--help"]:
        print("Usage: scripts/test.py full [Cargo options]\n"
              "       scripts/test.py -p PACKAGE [Cargo test options]\n\n"
              "Module implementations have their own test runner in Proteus-modules.\n"
              "Use PROTEUS_MODULES_SOURCE or PROTEUS_TEST_REFERENCE_MODULE to override fixtures.")
        return 0 if args else 2
    full = args[0] == "full"
    if full:
        args = ["--workspace", "--no-fail-fast", *args[1:]]
    elif not selected_packages(args):
        raise SystemExit("Select -p PACKAGE, or full for the harness workspace gate")
    env = test_env()
    start = time.monotonic()
    core = full or "proteus-core" in selected_packages(args)
    if core:
        if env.get("PROTEUS_TEST_REFERENCE_MODULE"):
            path = Path(env["PROTEUS_TEST_REFERENCE_MODULE"]).resolve()
            if not path.is_file():
                raise SystemExit(f"Missing process fixture: {path}")
            env["PROTEUS_TEST_REFERENCE_MODULE"] = str(path)
        else:
            env["PROTEUS_TEST_REFERENCE_MODULE"] = build(env=env)
    if core and (full or ("--lib" not in args and
                         ("--test" not in args or any(name in args for name in ("module_swap", "hook_runtime", "component_sandbox"))))):
        if not env.get("PROTEUS_TEST_DCP_MODULE"):
            dcp = resolve("modules") / "hook/dcp"
            for command in (["npm", "ci", "--ignore-scripts"], ["npm", "run", "build"]):
                subprocess.run(command, cwd=dcp, env=env, check=True)
            env["PROTEUS_TEST_DCP_MODULE"] = str(dcp / "dist/worker.js")
    prepared = time.monotonic()
    print(f"Preparation: {prepared-start:.2f}s", file=sys.stderr, flush=True)
    status = subprocess.call(["cargo", "test", "--locked", *args], cwd=ROOT, env=env)
    print(f"Test phase: {time.monotonic()-prepared:.2f}s; total: {time.monotonic()-start:.2f}s", file=sys.stderr)
    return status


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

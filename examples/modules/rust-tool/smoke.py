#!/usr/bin/env python3
"""Check a built standalone Rust tool using the public component conformance CLI.

This is a test runner, not a Python runtime dependency of the module or SDK.
No compilation, installation, network requests or private reference modules.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--module", type=Path, required=True)
    parser.add_argument("--conformance", type=Path, required=True)
    args = parser.parse_args()
    module = args.module.resolve(strict=True)
    conformance = args.conformance.resolve(strict=True)
    export = {
        "slot": "tool", "module_id": "example.echo", "contract_version": "v6",
        "module_config": {"prefix": "hello: "},
    }

    def probe(method, params, *, succeeds=True):
        command = [
            str(conformance), "--component-id", "external-rust-tool",
            "--export", json.dumps(export), "--probe-export", "tool/example.echo",
            "--probe-method", method, "--probe-params", json.dumps(params),
            "--", str(module),
        ]
        result = subprocess.run(command, text=True, capture_output=True, timeout=35)
        if not succeeds:
            if result.returncode == 0:
                raise AssertionError("invalid invocation unexpectedly succeeded")
            return
        if result.returncode:
            raise RuntimeError(result.stderr)
        report = json.loads(result.stdout)
        assert report["handshake"] == "success", report
        assert report["probe"]["terminal"] == "success", report
        return report["probe"]["result"]["result"]

    definitions = probe("list", None)
    assert len(definitions) == 1 and definitions[0]["spec"]["name"] == "echo"
    with tempfile.TemporaryDirectory(prefix="proteus-rust-tool-") as workdir:
        invocation = {
            "call": {
                "id": "sdk-smoke", "name": "echo", "args": {"text": "world"},
                "surface": "function", "raw_arguments": None,
            },
            "cwd": workdir,
            "attribution": {
                "execution_id": "00000000-0000-4000-8000-000000000001", "agent": None,
            },
            "skills": {"disabled": [], "packages": []},
        }
        result = probe("invoke", invocation)
        assert result["call_id"] == "sdk-smoke" and result["ok"] is True, result
        assert result["output"] == "hello: world", result
        invocation["call"]["args"] = {"text": 42}
        probe("invoke", invocation, succeeds=False)
    print("Rust tool: strict wire v3/tool v6 handshake, list, invocation and schema error passed")


if __name__ == "__main__":
    main()

"""Harbor installed-agent bridge to the pinned, native Proteus assembly.

No task, verifier or oracle files are uploaded by this adapter. Harbor owns
task setup, deadlines, artifact transfer and the separate verifier.
"""
import hashlib
import json
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import time
import tomllib
from types import SimpleNamespace

from harbor.agents.installed.base import BaseInstalledAgent

import adapters
from runner import check_pipeline, remove_auth


CORE = adapters.CORE
DEFAULT_BINARY = CORE.parent / "Proteus-desktop/build/Proteus/bin/proteus"
ENVIRONMENT = {
    "HOME": "/state/home",
    "CODEX_HOME": "/state/home/.codex",
    "XDG_CONFIG_HOME": "/state/home/.config",
    "XDG_DATA_HOME": "/state/home/.local/share",
    "XDG_CACHE_HOME": "/state/home/.cache",
    "XDG_STATE_HOME": "/state/home/.local/state",
    "PATH": "/runtime/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    "TMPDIR": "/tmp",
}
REMOTE_AUTH = "/state/home/.local/share/opencode/opencode.db"


def sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def prepare_agent_binaries(binary, config, destination, runtime_root="/runtime"):
    """Bundle only agent executables and their ABI, leaving task libraries alone."""
    commands = {binary.name}
    for component in tomllib.loads(config.read_text()).get("components", {}).values():
        commands.add(component["command"])
    bin_dir, abi_dir = destination / "bin", destination / "abi"
    bin_dir.mkdir(parents=True)
    abi_dir.mkdir()
    patcher = Path(sys.executable).parent / "patchelf"
    if not patcher.is_file():
        patcher = Path(shutil.which("patchelf") or "")
    if not patcher.is_file():
        raise ValueError("Install pinned patchelf==0.19.3.0 in the Harbor virtualenv")
    receipt = {"executables": {}, "libraries": {}, "patched_executables": {},
               "patcher_sha256": sha256(patcher),
               "patcher_version": subprocess.check_output([str(patcher), "--version"], text=True).strip()}
    for name in sorted(commands):
        if Path(name).name != name:
            raise ValueError("Pilot component commands must be portable-bin basenames")
        original, installed = binary.parent / name, bin_dir / name
        shutil.copy2(original, installed)
        receipt["executables"][name] = sha256(original)
        with original.open("rb") as stream:
            if stream.read(4) != b"\x7fELF":
                continue
        program = subprocess.check_output(["readelf", "-l", str(original)], text=True)
        interpreter = re.search(r"Requesting program interpreter: ([^\]]+)", program)
        if not interpreter:  # Static executables need no loader override.
            continue
        libraries = subprocess.check_output(["ldd", str(original)], text=True)
        if "not found" in libraries:
            raise ValueError(f"Unresolved installed agent dependency: {name}")
        paths = re.findall(r"(?:=>\s+|^\s*)(/\S+)\s+\(", libraries, re.M)
        paths.append(interpreter[1])
        for value in paths:
            path = Path(value)
            digest = sha256(path)
            previous = receipt["libraries"].get(path.name)
            if previous and previous != digest:
                raise ValueError(f"Conflicting agent ABI dependency: {path.name}")
            if not previous:
                shutil.copy2(path, abi_dir / path.name, follow_symlinks=True)
                receipt["libraries"][path.name] = digest
        # Invoking ld-linux explicitly would make current_exe() resolve to the
        # loader and break Proteus CLI's app-server re-exec. Patch only this
        # disposable copy so /proc/self/exe stays the real agent executable.
        subprocess.run([str(patcher), "--set-interpreter",
                        runtime_root + "/abi/" + Path(interpreter[1]).name,
                        "--set-rpath", runtime_root + "/abi", str(installed)], check=True)
        receipt["patched_executables"][name] = sha256(installed)
    return bin_dir, abi_dir, receipt


def is_deadline_error(error):
    """Harbor's local container runtime wraps its exec deadline in RuntimeError."""
    return isinstance(error, TimeoutError) or (
        isinstance(error, RuntimeError)
        and re.fullmatch(r"Command timed out after \d+(?:\.\d+)? seconds", str(error)) is not None)


class ProteusAgent(BaseInstalledAgent):
    """`-a tb4_agent:ProteusAgent`; import with this directory on PYTHONPATH."""

    DEFAULT_SETUP_TIMEOUT_SEC = 900.0

    def __init__(self, *args, binary=DEFAULT_BINARY,
                 base_config=CORE / "configs/config.toml", reasoning="high",
                 timeout=1200, disable_subagents=True, inspect_only=False, **kwargs):
        # This native catalog/auth path must not be rewritten by LiteLLM or
        # trigger its remote pricing-map fetch during agent construction.
        kwargs.setdefault("skip_model_parsing", True)
        super().__init__(*args, **kwargs)
        self.binary = Path(binary).expanduser().resolve()
        self.base_config = Path(base_config).expanduser().resolve()
        self.reasoning = reasoning
        self.timeout = int(timeout)
        self.disable_subagents = disable_subagents
        self.inspect_only = inspect_only
        if not self.binary.is_file() or not self.base_config.is_file():
            raise ValueError("A built Proteus binary and a self-contained config are required")
        if self.model_name not in (None, "opencode-go/deepseek-v4.1-flash"):
            raise ValueError("This pilot pins opencode-go/deepseek-v4.1-flash")
        if reasoning not in ("low", "high", "max") or not 1 <= self.timeout <= 28800:
            raise ValueError("Expected Flash effort low/high/max and timeout 1..28800")
        if not isinstance(disable_subagents, bool) or not isinstance(inspect_only, bool):
            raise ValueError("disable_subagents and inspect_only must be booleans")
        # Existing canonical metrics require exactly one root journal. Avoid
        # silently excluding child usage from the first, inexpensive pilot.
        if not disable_subagents:
            raise ValueError("This pilot requires disable_subagents=true")
        self.runtime = self.logs_dir / "runtime"
        self.inner = None
        self.receipt = None
        self.abi_receipt = None

    @staticmethod
    def name():
        return "proteus-native"

    def get_version_command(self):
        return "/runtime/bin/proteus --version"

    async def install(self, environment):
        # These are agent prerequisites, not changes to the task implementation
        # or verifier. Debian/Ubuntu are the bases of all seven pilot tasks.
        result = await self.exec_as_root(
            environment,
            command=("apt-get update && DEBIAN_FRONTEND=noninteractive "
                     "apt-get install -y --no-install-recommends "
                     "ca-certificates git ripgrep python3 libgcc-s1 && "
                     "rm -rf /var/lib/apt/lists/*"),
            timeout_sec=600,
        )
        setup = self.logs_dir / "setup"
        setup.mkdir(parents=True, exist_ok=True)
        (setup / "dependencies.log").write_text((result.stdout or "") + (result.stderr or ""))
        versions = await self.exec_as_root(
            environment,
            command="dpkg-query -W -f='${binary:Package}\t${Version}\n' "
                    "ca-certificates git ripgrep python3 libgcc-s1",
            timeout_sec=30,
        )
        (setup / "dependencies.tsv").write_text(versions.stdout or "")

    async def _remove_remote_auth(self, environment):
        paths = [REMOTE_AUTH + suffix for suffix in ("", "-wal", "-shm")]
        result = await environment.exec("rm -f -- " + shlex.join(paths), user="root", timeout_sec=30)
        if result.return_code:
            raise RuntimeError("Could not remove disposable native credentials")

    async def configure(self, environment):
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        self.runtime.mkdir(mode=0o700)
        case = self.logs_dir / "input"
        case.mkdir()
        (case / "prompt.txt").write_text("Harbor supplies the unchanged task instruction at run time.\n")
        args = SimpleNamespace(
            agent="proteus", model="opencode-go/deepseek-v4.1-flash",
            reasoning=self.reasoning, binary=self.binary, base_config=self.base_config,
            timeout=self.timeout, disable_subagents=self.disable_subagents,
            case_dir=case, auth_file=None,
        )
        try:
            self.inner, mounts = adapters.prepare(args, self.runtime)
            self.inner[self.inner.index("--cwd") + 1] = "/app"
            bin_dir, abi_dir, self.abi_receipt = prepare_agent_binaries(
                self.binary, self.base_config, self.logs_dir / "installed-agent")
            (self.logs_dir / "setup").mkdir(exist_ok=True)
            (self.logs_dir / "setup/agent-abi.json").write_text(
                json.dumps(self.abi_receipt, indent=2) + "\n")
            mounts = [(bin_dir if destination == "/runtime/bin" else source,
                       destination, *flags) for source, destination, *flags in mounts]
            mounts.append((abi_dir, "/runtime/abi"))
            await environment.exec("mkdir -p /state /runtime", user="root")
            await environment.upload_dir(self.runtime, "/state")
            # A disposable container replaces bubblewrap. The same prepared
            # binary/config/runtime trees are copied to the same absolute paths.
            for source, destination, *_ in mounts:
                await environment.exec(
                    "mkdir -p " + shlex.quote(str(Path(destination).parent)), user="root")
                if source.is_dir():
                    await environment.upload_dir(source, destination)
                else:
                    await environment.upload_file(source, destination)
            await environment.exec("chmod -R go-rwx /state/home", user="root")
            node = "/runtime/share/opencode-runtime/workflow/opencode-runtime/node_modules/node-linux-x64/bin/node"
            version = await self.exec_as_root(environment, shlex.quote(node) + " --version", timeout_sec=30)
            (self.logs_dir / "setup/node-version.txt").write_text(version.stdout or "")
        except BaseException:
            await self._remove_remote_auth(environment)
            raise
        finally:
            # Never retain a copy of the native key in published trial logs.
            remove_auth(self.runtime)

    async def _collect(self, environment, context):
        errors = []
        try:
            await environment.download_dir("/runtime/sessions", self.runtime / "sessions")
            # Verify the effective config actually used inside the container,
            # rather than asserting only that its host staging copy was intact.
            await environment.download_file("/runtime/configs/eval.json", self.runtime / "configs/eval.json")
            for name in ("stdout.jsonl", "stderr.log", "events.jsonl",
                         "cli-smoke.stdout", "cli-smoke.stderr"):
                remote = "/state/" + name
                if await environment.is_file(remote):
                    await environment.download_file(remote, self.logs_dir / name)
        except Exception as exc:
            errors.append({"phase": "collect", "type": type(exc).__name__})
        finally:
            try:
                await self._remove_remote_auth(environment)
            except Exception as exc:
                errors.append({"phase": "credential_cleanup", "type": type(exc).__name__})
            remove_auth(self.runtime)
        if self.receipt is None:
            return
        self.receipt["collection_errors"] = errors
        self.receipt["auth_removed"] = not any(e["phase"] == "credential_cleanup" for e in errors)
        if not self.inspect_only:
            try:
                self.receipt["metrics"] = adapters.metrics("proteus", self.logs_dir)
                check_pipeline(self.receipt)
            except (ValueError, KeyError, OSError) as exc:
                self.receipt["metrics"] = {"error": type(exc).__name__}
                self.receipt["pipeline_ok"] = False
        else:
            self.receipt["pipeline_ok"] = False
            self.receipt["prepared_only"] = True
        self.receipt["pipeline_ok"] = self.receipt["pipeline_ok"] and not errors
        (self.logs_dir / "native-result.json").write_text(json.dumps(self.receipt, indent=2) + "\n")
        usage = self.receipt.get("metrics", {}).get("provider_usage", {})
        context.n_input_tokens = usage.get("input_tokens")
        context.n_cache_tokens = usage.get("cached_input_tokens")
        context.n_output_tokens = usage.get("output_tokens")
        context.metadata = {"native_result": "native-result.json",
                            "pipeline_ok": self.receipt["pipeline_ok"],
                            "inspect_only": self.inspect_only}

    async def run(self, instruction, environment, context):
        if self.inner is None:
            raise RuntimeError("Harbor must configure the agent before run")
        inner = list(self.inner)
        inner[-1:] = (["inspect", "plan", "--format=json"] if self.inspect_only else [instruction])
        (self.logs_dir / "input/prompt.txt").write_text(instruction)
        self.receipt = {
            "format": 1, "agent": "proteus", "model": "opencode-go/deepseek-v4.1-flash",
            "reasoning": self.reasoning, "expected_turns": 1,
            "binary_sha256": sha256(self.binary), "base_config_sha256": sha256(self.base_config),
            "prompt_sha256": hashlib.sha256(instruction.encode()).hexdigest(),
            "timeout_seconds": self.timeout, "disable_subagents": self.disable_subagents,
            "isolation": "Harbor task container; separate upstream verifier",
            "environment_names": sorted(ENVIRONMENT), "timed_out": False,
            "agent_abi": self.abi_receipt,
        }
        started = time.monotonic()
        try:
            result = await environment.exec(
                command=shlex.join(inner) + " > /state/stdout.jsonl 2> /state/stderr.log",
                cwd="/app", env=ENVIRONMENT, timeout_sec=self.timeout,
            )
            self.receipt["returncode"] = result.return_code
            if self.inspect_only and result.return_code == 0:
                # With no task and EOF stdin the product CLI launches its
                # app-server, requests ConfigSummary, then shuts it down.
                # This exercises current_exe() re-exec without sending a turn.
                smoke = await environment.exec(
                    command=shlex.join(self.inner[:-1])
                            + " </dev/null > /state/cli-smoke.stdout 2> /state/cli-smoke.stderr",
                    cwd="/app", env=ENVIRONMENT, timeout_sec=60,
                )
                self.receipt["cli_app_server_smoke_returncode"] = smoke.return_code
                self.receipt["returncode"] = smoke.return_code
        except BaseException as exc:
            self.receipt["returncode"] = None
            self.receipt["interruption_type"] = type(exc).__name__
            self.receipt["timed_out"] = is_deadline_error(exc)
            if self.receipt["timed_out"]:
                self.receipt["failure_kind"] = "agent_deadline"
            raise
        finally:
            self.receipt["wall_seconds"] = time.monotonic() - started
            await self._collect(environment, context)
        if self.receipt["returncode"]:
            raise RuntimeError("Proteus CLI failed; see native-result.json and stderr.log")

"""Offline lifecycle checks for the Harbor bridge; no provider calls."""
import asyncio
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from harbor.models.agent.context import AgentContext

import tb4_agent


class Environment:
    def __init__(self, *, upload_error=False, interrupted=False):
        self.commands = []
        self.uploads = []
        self.upload_error = upload_error
        self.interrupted = interrupted

    async def exec(self, command, **kwargs):
        self.commands.append((command, kwargs))
        if "> /state/stdout.jsonl" in command and self.interrupted:
            if isinstance(self.interrupted, BaseException):
                raise self.interrupted
            raise asyncio.CancelledError()
        return SimpleNamespace(return_code=0, stdout="", stderr="")

    async def upload_dir(self, source, target):
        self.uploads.append((source, target))
        if self.upload_error:
            raise OSError("synthetic upload failure")

    async def upload_file(self, source, target):
        self.uploads.append((source, target))

    async def download_dir(self, source, target):
        Path(target).mkdir(parents=True, exist_ok=True)

    async def download_file(self, source, target):
        if source.endswith("eval.json"):
            Path(target).write_text("{}")

    async def is_file(self, path):
        return False


class Lifecycle(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        root = tb4_agent.CORE / "target/performance/tb4-agent-tests"
        root.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=root)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.binary = self.root / "proteus"
        self.config = self.root / "agent.toml"
        self.binary.write_text("synthetic binary")
        self.config.write_text('model = "cheap"\n')

    def agent(self, **kwargs):
        return tb4_agent.ProteusAgent(
            logs_dir=self.root / "agent", binary=self.binary,
            base_config=self.config,
            model_name="opencode-go/deepseek-v4.1-flash", **kwargs,
        )

    def prepare(self, args, runtime):
        credential = runtime / "home/.local/share/opencode/opencode.db"
        credential.parent.mkdir(parents=True)
        credential.write_text("synthetic private value")
        (runtime / "configs").mkdir()
        (runtime / "configs/eval.json").write_text("{}")
        (runtime / "sessions").mkdir()
        return ["/runtime/bin/proteus", "--config", "/runtime/configs/eval.json",
                "--cwd", "/workspace", "placeholder"], [(runtime / "configs", "/runtime/configs")]

    async def test_configure_removes_private_host_credentials_on_upload_failure(self):
        agent = self.agent()
        environment = Environment(upload_error=True)
        with patch.object(tb4_agent.adapters, "prepare", side_effect=self.prepare):
            with self.assertRaisesRegex(OSError, "synthetic upload failure"):
                await agent.configure(environment)
        self.assertFalse((agent.runtime / "home/.local/share/opencode/opencode.db").exists())
        self.assertTrue(any(command.startswith("rm -f -- /state/home/")
                            for command, _ in environment.commands))

    async def test_cancellation_collects_receipt_and_cleans_credentials(self):
        agent = self.agent()
        environment = Environment(interrupted=True)
        with patch.object(tb4_agent.adapters, "prepare", side_effect=self.prepare):
            await agent.configure(environment)
        # No model exchange exists, which must never be counted as clean success.
        with patch.object(tb4_agent.adapters, "metrics", return_value={"error": "no exchange"}):
            with self.assertRaises(asyncio.CancelledError):
                await agent.run("unchanged task", environment, AgentContext())
        result = json.loads((agent.logs_dir / "native-result.json").read_text())
        self.assertFalse(result["pipeline_ok"])
        self.assertTrue(result["auth_removed"])
        self.assertEqual(result["interruption_type"], "CancelledError")
        self.assertNotIn("synthetic private value", json.dumps(result))
        self.assertTrue(environment.commands[-1][0].startswith("rm -f -- /state/home/"))

    async def test_inspect_never_invokes_inference_or_metrics(self):
        agent = self.agent(inspect_only=True)
        environment = Environment()
        with patch.object(tb4_agent.adapters, "prepare", side_effect=self.prepare):
            await agent.configure(environment)
        context = AgentContext()
        with patch.object(tb4_agent.adapters, "metrics", side_effect=AssertionError("paid run")):
            await agent.run("instruction must not be sent", environment, context)
        commands = [command for command, _ in environment.commands if "> /state/stdout.jsonl" in command]
        self.assertEqual(len(commands), 1)
        self.assertIn("--cwd /app inspect plan --format=json", commands[0])
        self.assertNotIn("instruction must not be sent", commands[0])
        result = json.loads((agent.logs_dir / "native-result.json").read_text())
        self.assertTrue(result["prepared_only"])
        self.assertFalse(result["pipeline_ok"])
        self.assertIsNone(context.n_input_tokens)

    async def test_rejects_children_until_usage_aggregation_exists(self):
        with self.assertRaisesRegex(ValueError, "disable_subagents=true"):
            self.agent(disable_subagents=False)

    async def test_runtime_wrapped_deadline_is_not_an_infrastructure_failure(self):
        agent = self.agent()
        environment = Environment(interrupted=RuntimeError("Command timed out after 1200 seconds"))
        with patch.object(tb4_agent.adapters, "prepare", side_effect=self.prepare):
            await agent.configure(environment)
        with patch.object(tb4_agent.adapters, "metrics", return_value={"error": "incomplete exchange"}):
            with self.assertRaisesRegex(RuntimeError, "Command timed out"):
                await agent.run("unchanged task", environment, AgentContext())
        result = json.loads((agent.logs_dir / "native-result.json").read_text())
        self.assertTrue(result["timed_out"])
        self.assertEqual(result["failure_kind"], "agent_deadline")
        self.assertFalse(tb4_agent.is_deadline_error(RuntimeError("synthetic network failure")))

    async def test_agent_abi_bundle_keeps_binary_and_task_library_environment_separate(self):
        shutil.copy2("/usr/bin/readlink", self.binary)
        before = self.binary.read_bytes()
        installed = self.root / "installed-agent"
        bins, libraries, receipt = tb4_agent.prepare_agent_binaries(
            self.binary, self.config, installed, runtime_root=str(installed))
        self.assertEqual(self.binary.read_bytes(), before)
        self.assertNotEqual((bins / "proteus").read_bytes(), before)
        self.assertEqual(receipt["executables"]["proteus"], tb4_agent.sha256(self.binary))
        self.assertEqual(receipt["patched_executables"]["proteus"], tb4_agent.sha256(bins / "proteus"))
        # A subprocess resolving itself must see the executable, not ld-linux;
        # the real CLI uses precisely this property to spawn its app-server.
        observed = subprocess.check_output([str(bins / "proteus"), "/proc/self/exe"], text=True).strip()
        self.assertEqual(observed, str(bins / "proteus"))
        self.assertTrue(list(libraries.glob("ld-linux*")))


if __name__ == "__main__":
    unittest.main()

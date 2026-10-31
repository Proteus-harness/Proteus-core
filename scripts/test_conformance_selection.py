"""Runner-selection regression source; no Cargo or fixture process is started."""
import unittest
from unittest.mock import patch

import test as runner


class ConformanceSelection(unittest.TestCase):
    def invoke(self, arguments):
        with (patch.object(runner, "test_env", return_value={}),
              patch.object(runner, "build_packages", return_value={}) as packages,
              patch.object(runner, "build", return_value="/fixture/reference") as reference,
              patch.object(runner.subprocess, "call", return_value=0) as cargo):
            self.assertEqual(runner.main(arguments), 0)
        return cargo.call_args.args[0], packages.call_count, reference.call_count

    def test_full_preserves_conformance_binary_coverage(self):
        command, packages, reference = self.invoke(["full", "--", "--nocapture"])
        self.assertEqual(command, [
            "cargo", "test", "--locked", "--workspace", "--no-fail-fast",
            "--features", "proteus-module-protocol/conformance-cli",
            "--", "--nocapture",
        ])
        self.assertEqual((packages, reference), (1, 1))

    def test_selected_protocol_stays_library_only_by_default(self):
        command, packages, reference = self.invoke(["-p", "proteus-module-protocol"])
        self.assertEqual(command, [
            "cargo", "test", "--locked", "-p", "proteus-module-protocol",
        ])
        self.assertEqual((packages, reference), (0, 0))

    def test_explicit_cli_feature_is_forwarded_without_core_fixtures(self):
        arguments = ["-p", "proteus-module-protocol", "--features", "conformance-cli"]
        command, packages, reference = self.invoke(arguments)
        self.assertEqual(command, ["cargo", "test", "--locked", *arguments])
        self.assertEqual((packages, reference), (0, 0))


if __name__ == "__main__":
    unittest.main()

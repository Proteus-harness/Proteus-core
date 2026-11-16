"""Caller-owned path identity at the packager command boundary; no child runs."""
from contextlib import chdir
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import Mock, patch


HERE = Path(__file__).resolve().parent


def load_builder():
    sources = types.ModuleType('sources')
    sources.resolve = Mock(side_effect=AssertionError('Unexpected source resolution'))
    path = HERE / 'build_module.py'
    spec = importlib.util.spec_from_file_location('_path_identity_builder', path)
    module = importlib.util.module_from_spec(spec)
    with patch.dict(sys.modules, {'sources': sources, spec.name: module}):
        spec.loader.exec_module(module)
    if Path(module.__file__).resolve() != path:
        raise AssertionError('Wrong builder module')
    return module


class BuildModulePathTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='module-path-identity-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.caller = self.root / 'caller'
        self.caller.mkdir()
        self.modules = self.root / 'different-modules-checkout'
        self.modules.mkdir()
        self.builder = load_builder()
        self.builder.resolve = Mock(return_value=self.modules)
        patcher = patch.object(self.builder.subprocess, 'check_output', return_value='')
        self.output = patcher.start()
        self.addCleanup(patcher.stop)

    def command(self):
        self.output.assert_called_once()
        args, kwargs = self.output.call_args
        self.assertEqual(kwargs['cwd'], self.modules)
        self.assertTrue(kwargs['text'])
        return args[0], kwargs

    def test_relative_bundle_and_target_dir_use_callers_directory(self):
        env = {'SYNTHETIC_OPTION': 'value'}
        with chdir(self.caller):
            result = self.builder.build_packages(
                'release', 'target-cache', env, Path('delivered runtime'), reuse=True,
            )
        command, kwargs = self.command()
        self.assertIsNone(result)
        self.assertEqual(command, [
            sys.executable, str(self.modules / 'package.py'), '--profile', 'release',
            '--bundle', str(self.caller / 'delivered runtime'),
            '--target-dir', str(self.caller / 'target-cache'), '--no-build',
        ])
        self.assertIs(kwargs['env'], env)
        self.assertFalse((self.caller / 'delivered runtime').exists())
        self.assertFalse((self.modules / 'delivered runtime').exists())

    def test_dot_and_parent_bundle_paths_are_resolved_before_foreign_cwd(self):
        for supplied, expected in ((Path('.'), self.caller),
                                   (Path('../output'), self.root / 'output'),
                                   ('string-output', self.caller / 'string-output')):
            with self.subTest(supplied=supplied):
                self.output.reset_mock()
                with chdir(self.caller):
                    self.builder.build_packages(destination=supplied)
                command, _ = self.command()
                self.assertEqual(command[command.index('--bundle') + 1], str(expected))

    def test_absolute_unicode_bundle_stays_one_unchanged_argument(self):
        destination = self.root / 'absolute bundle 候補'
        with chdir(self.caller):
            self.builder.build_packages(destination=destination)
        command, _ = self.command()
        self.assertEqual(command, [
            sys.executable, str(self.modules / 'package.py'), '--profile', 'dev',
            '--bundle', str(destination),
        ])
        self.assertFalse(destination.exists())

    def test_artifact_listing_keeps_existing_json_return_contract(self):
        artifacts = {'fixture': str(self.root / 'unused-binary')}
        self.output.return_value = json.dumps(artifacts)
        target = self.root / 'absolute-target'
        with chdir(self.caller):
            result = self.builder.build_packages(target_dir=target)
        command, kwargs = self.command()
        self.assertEqual(result, artifacts)
        self.assertEqual(command, [
            sys.executable, str(self.modules / 'package.py'), '--profile', 'dev',
            '--print-artifacts', '--target-dir', str(target),
        ])
        self.assertIsNone(kwargs['env'])

    def test_packager_failure_is_not_swallowed(self):
        error = subprocess.CalledProcessError(7, ['synthetic-command'])
        self.output.side_effect = error
        with chdir(self.caller), self.assertRaises(subprocess.CalledProcessError) as caught:
            self.builder.build_packages(destination=Path('out'))
        self.assertIs(caught.exception, error)

    def test_node_workers_use_the_same_resolved_source_and_bundle(self):
        env = {'SYNTHETIC_OPTION': 'value'}
        with chdir(self.caller), patch.object(self.builder.subprocess, 'run') as run:
            self.builder.build_packages(destination=Path('staged runtime'), env=env,
                                        node_runtimes=True)
        command, _ = self.command()
        destination = str(self.caller / 'staged runtime')
        self.assertEqual(command[-2:], ['--bundle', destination])
        self.builder.resolve.assert_called_once_with('modules')
        self.assertEqual(run.call_count, 2)
        for call, relative in zip(run.call_args_list,
                                  ('tool/pi-tools/install.py', 'workflow/opencode-runtime/install.py')):
            self.assertEqual(call.args[0], [sys.executable, str(self.modules / relative),
                                           '--prefix', destination])
            self.assertEqual(call.kwargs, {'cwd': self.modules, 'env': env, 'check': True})

    def test_node_workers_require_a_bundle_before_resolving_sources(self):
        with self.assertRaisesRegex(ValueError, 'requires --bundle'):
            self.builder.build_packages(node_runtimes=True)
        self.builder.resolve.assert_not_called()
        self.output.assert_not_called()


if __name__ == '__main__':
    unittest.main()

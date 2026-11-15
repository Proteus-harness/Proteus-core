"""Loader-only controls with synthetic modules; not the archive grading suite."""
from contextlib import contextmanager
import importlib
from pathlib import Path
import runpy
import shutil
import sys
import tempfile
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent


class GraderLoaderTests(unittest.TestCase):
    @contextmanager
    def grader(self, root, case='journal-segments'):
        code = root/'examples/performance/history-scale'
        code.mkdir(parents=True)
        shutil.copyfile(HERE.parent/'history-scale/journal.py', code/'journal.py')
        grader = HERE/'long_repo_fixtures'/case/'grader/test_outputs.py'
        # Use the actual loader functions. Only pytest's declaration decorator is
        # stubbed; no pytest collection or general archive/CLI grading is run.
        pytest_stub = SimpleNamespace(mark=SimpleNamespace(
            parametrize=lambda *args, **kwargs: lambda function: function))
        with patch.dict(sys.modules), patch.object(sys, 'path', sys.path.copy()):
            sys.modules['pytest'] = pytest_stub
            for name in ('journal', 'archive_under_test', 'usage_under_test', 'phase_under_test'):
                sys.modules.pop(name, None)
            sys.path.insert(0, str(code))
            journal = importlib.import_module('journal')
            self.assertEqual(Path(journal.__file__), code/'journal.py')
            scope = runpy.run_path(str(grader))
            entry = scope['archiver' if case == 'journal-segments' else 'load']
            entry.__globals__.update(ROOT=root, CODE=code)
            yield scope, code

    def test_archive_supports_postponed_annotation_dataclass(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.grader(Path(directory)) as (scope, code):
                (code/'archive.py').write_text(
                    'from __future__ import annotations\nfrom dataclasses import dataclass\n'
                    '@dataclass\nclass Row:\n    value: int\n'
                    'def archive():\n    return Row(7)\n')
                value = scope['archiver']()()
                self.assertEqual(value.value, 7)
                self.assertIs(sys.modules[value.__class__.__module__].Row, type(value))

    def test_existing_consumer_loader_supports_dataclass(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.grader(root) as (scope, code):
                (code/'archive.py').write_text('def archive(source, output):\n    output.mkdir()\n')
                measurements = root/'examples/performance/code-mode/measurements.py'
                measurements.parent.mkdir()
                measurements.write_text(
                    'from __future__ import annotations\nfrom dataclasses import dataclass, asdict\n'
                    '@dataclass\nclass Row:\n    all_tool_calls: int\n'
                    'def measure(session):\n    return asdict(Row(0))\n')
                # This one real consumer test body uses only our trivial archive
                # and measurement fixtures, plus the unchanged journal helpers.
                # Passing it is not evidence of a valid archive implementation.
                scope['test_existing_consumers_and_public_helpers'](root)
                self.assertEqual(sys.modules['usage_under_test'].Row(0).all_tool_calls, 0)

    def test_success_reloads_fresh_module_without_mutating_previous_binding(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.grader(Path(directory)) as (scope, code):
                previous = ModuleType('archive_under_test')
                previous.value = 'previous'
                sys.modules['archive_under_test'] = previous
                source = code/'archive.py'
                source.write_text('def archive():\n    return 1\n')
                first = scope['archiver']()
                loaded = sys.modules['archive_under_test']
                self.assertIsNot(loaded, previous)
                self.assertEqual(previous.value, 'previous')
                self.assertEqual(first(), 1)
                source.write_text('def archive():\n    return 22\n')
                second = scope['archiver']()
                self.assertIsNot(sys.modules['archive_under_test'], loaded)
                self.assertEqual(first(), 1)
                self.assertEqual(second(), 22)
                self.assertIs(sys.modules['archive_under_test'].archive, second)

    def test_failed_load_removes_new_registration_and_propagates_exception(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.grader(Path(directory)) as (scope, code):
                fixture = ModuleType('loader_error_fixture')
                sys.modules[fixture.__name__] = fixture
                for error in (ValueError('synthetic'), SystemExit('synthetic')):
                    with self.subTest(error=type(error).__name__):
                        fixture.error = error
                        (code/'archive.py').write_text(
                            'import sys\nfrom loader_error_fixture import error\n'
                            'assert sys.modules[__name__].__dict__ is globals()\nraise error\n')
                        with self.assertRaises(type(error)) as caught:
                            scope['archiver']()
                        self.assertIs(caught.exception, error)
                        self.assertNotIn('archive_under_test', sys.modules)

    def test_failed_load_restores_previous_module_or_none(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.grader(Path(directory)) as (scope, code):
                fixture = ModuleType('loader_error_fixture')
                sys.modules[fixture.__name__] = fixture
                previous = ModuleType('archive_under_test')
                previous.value = 'unchanged'
                (code/'archive.py').write_text(
                    'import sys\nfrom loader_error_fixture import error\n'
                    'assert sys.modules[__name__].__dict__ is globals()\nraise error\n')
                for binding in (previous, None):
                    for error in (ValueError('synthetic'), SystemExit('synthetic')):
                        with self.subTest(binding=binding, error=type(error).__name__):
                            sys.modules['archive_under_test'] = binding
                            fixture.error = error
                            with self.assertRaises(type(error)) as caught:
                                scope['archiver']()
                            self.assertIs(caught.exception, error)
                            self.assertIn('archive_under_test', sys.modules)
                            self.assertIs(sys.modules['archive_under_test'], binding)
                self.assertEqual(previous.value, 'unchanged')

    def test_existing_sampler_registration_control(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.grader(root, 'sampler-phase-report') as (scope, _):
                module = root/'examples/performance/common/phase_report.py'
                module.parent.mkdir()
                module.write_text(
                    'from __future__ import annotations\nfrom dataclasses import dataclass\n'
                    '@dataclass\nclass Row:\n    value: int\n'
                    'def summarize():\n    return Row(7)\n')
                value = scope['load']()()
                self.assertEqual(value.value, 7)
                self.assertIs(sys.modules[value.__class__.__module__].Row, type(value))


if __name__ == '__main__':
    unittest.main()

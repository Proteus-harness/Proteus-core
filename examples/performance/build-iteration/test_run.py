"""Exercise the benchmark protocol without invoking Rust or the real project."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

RUNNER = Path(__file__).with_name('run.py')


class BenchmarkProtocol(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='proteus-build-protocol-')
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.project, self.tools = self.root / 'project', self.root / 'tools'
        self.project.mkdir()
        self.tools.mkdir()
        self.source = self.project / 'source.rs'
        self.source.write_bytes(b'const MESSAGE: &str = "a file was started";\n')
        (self.project / 'Cargo.toml').write_text('[profile.fast]\nopt-level = 1\n')
        (self.project / 'Cargo.lock').write_text('# synthetic fixture\n')
        subprocess.run(['git', 'init', '-q', str(self.project)], check=True)
        self.git('add', '.')
        self.git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
                 'commit', '-qm', 'fixture')
        fake = '''#!/usr/bin/env python3
import json, os, pathlib, sys
if '-V' in sys.argv or '-Vv' in sys.argv:
    print('synthetic toolchain')
else:
    edited = b'a diff file was started' in pathlib.Path(os.environ['FAKE_SOURCE']).read_bytes()
    print(json.dumps(dict(reason='compiler-artifact', package_id='fake#0.1',
                         target=dict(name='fake'), fresh=not edited)))
    sys.exit(7 if edited and os.environ.get('FAKE_FAIL_EDIT') else 0)
'''
        for name in ('cargo', 'rustc'):
            tool = self.tools / name
            tool.write_text(fake)
            tool.chmod(0o755)
        self.environment = dict(os.environ, PATH=str(self.tools) + ':' + os.environ['PATH'],
                                FAKE_SOURCE=str(self.source))

    def git(self, *args):
        subprocess.run(['git', '-C', str(self.project), *args], check=True,
                       capture_output=True)

    def run_benchmark(self):
        return subprocess.run(['python3', str(RUNNER), '--project', str(self.project),
                               '--output', str(self.root / 'output'), '--edit-file', 'source.rs',
                               '--old', 'a file was started', '--new', 'a diff file was started',
                               '--repeats', '2'], env=self.environment, capture_output=True)

    def test_phases_restore_bytes_and_existing_output_is_preserved(self):
        original = self.source.read_bytes()
        result = self.run_benchmark()
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        path = self.root / 'output/report.json'
        report = json.loads(path.read_text())
        self.assertEqual([row['phase'] for row in report['phases']],
                         ['warmup', '1-noop', '1-edit', '1-restore', '2-noop', '2-edit', '2-restore'])
        self.assertEqual(self.source.read_bytes(), original)
        self.assertEqual(report['summary']['edit']['wall_seconds']['median'],
                         sum(row['wall_seconds'] for row in report['phases']
                             if row['phase'].endswith('-edit')) / 2)
        saved = path.read_bytes()
        self.assertNotEqual(self.run_benchmark().returncode, 0)
        self.assertEqual(path.read_bytes(), saved)

    def test_failed_edit_restores_original_and_keeps_failure_logs(self):
        original = self.source.read_bytes()
        self.environment['FAKE_FAIL_EDIT'] = '1'
        self.assertNotEqual(self.run_benchmark().returncode, 0)
        self.assertEqual(self.source.read_bytes(), original)
        report = json.loads((self.root / 'output/report.json').read_text())
        self.assertEqual(report['status'], 'failed')
        phase = json.loads((self.root / 'output/1-edit.json').read_text())
        self.assertEqual(phase['exit_code'], 7)
        self.assertFalse((self.root / 'output/1-restore.json').exists())

    def test_dirty_source_is_rejected_without_output_or_edits(self):
        dirty = self.source.read_bytes() + b'// user edit\n'
        self.source.write_bytes(dirty)
        self.assertNotEqual(self.run_benchmark().returncode, 0)
        self.assertEqual(self.source.read_bytes(), dirty)
        self.assertFalse((self.root / 'output').exists())


if __name__ == '__main__':
    unittest.main()

"""Import-style controls for the real grader probe, using a synthetic JSON store."""
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import unittest

PROBE = Path(__file__).parent / 'long_cases/ledger-recovery/grader/io_probe.py'
STORE = '''
import json
from pathlib import Path
class Store:
    @classmethod
    def open(cls, root):
        self = cls()
        self.root = Path(root)
        self.values = json.loads((self.root/'journal').read_text()) if (self.root/'journal').exists() else {}
        return self
    def get(self, key):
        return self.values.get(key)
    def put(self, key, value, event_id):
        self.root.mkdir(exist_ok=True)
        self.values[key] = value
        with (self.root/'journal').open('w') as stream:
            json.dump(self.values, stream)
            stream.flush()
            if not NO_APPEND_SYNC: sync(stream.fileno())
    def compact(self):
        paths = [self.root/'snapshot.tmp', self.root/'journal.tmp']
        try:
            for path in paths:
                with path.open('w') as stream:
                    json.dump(self.values, stream)
                    stream.flush()
                    if not NO_COMPACT_SYNC: sync(stream.fileno())
            replace(paths[0], self.root/'snapshot')
            replace(paths[1], self.root/'journal')
            fd = os.open(self.root, os.O_RDONLY | os.O_DIRECTORY)
            try:
                if not NO_DIR_SYNC: sync(fd)
            finally: os.close(fd)
        finally:
            if not LEAK_TEMP:
                for path in paths: path.unlink(missing_ok=True)
'''
IMPORTS = {
    'qualified': 'import os\ndef replace(*args): return os.replace(*args)\ndef sync(fd): return os.fsync(fd)\n',
    'direct': 'import os\nfrom os import replace, fsync as sync\n',
    'captured': ('import os\nfrom os import replace as _replace, fsync as _sync\n'
                 'def replace(*args, operation=_replace): return operation(*args)\n'
                 'def sync(fd, operation=_sync): return operation(fd)\n'),
}


class LedgerProbeTests(unittest.TestCase):
    def run_probe(self, style, probe, *, optimized=False, **flags):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            settings = '\n'.join(f'{name} = {flags.get(name, False)!r}' for name in
                                 ('NO_APPEND_SYNC', 'NO_COMPACT_SYNC', 'NO_DIR_SYNC', 'LEAK_TEMP'))
            (root/'ledger.py').write_text(IMPORTS[style] + settings + '\n' + textwrap.dedent(STORE))
            command = [sys.executable, '-B', *(['-O'] if optimized else []), str(PROBE.resolve()),
                       probe, str(root/'data')]
            return subprocess.run(command, cwd=root, capture_output=True, text=True, timeout=10)

    def test_public_os_contract_handles_qualified_direct_and_captured_imports(self):
        for style in IMPORTS:
            for probe in ('replace', 'fsync'):
                with self.subTest(style=style, probe=probe):
                    result = self.run_probe(style, probe)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_invalid_implementations_fail_with_the_expected_diagnostic(self):
        cases = [('fsync', 'NO_APPEND_SYNC', 'append did not fsync a file'),
                 ('fsync', 'NO_COMPACT_SYNC', 'compact did not fsync two files'),
                 ('fsync', 'NO_DIR_SYNC', 'compact did not fsync two files'),
                 ('replace', 'LEAK_TEMP', 'failed snapshot replacement changed existing bytes')]
        for style in IMPORTS:
            for probe, flag, diagnostic in cases:
                with self.subTest(style=style, flag=flag):
                    result = self.run_probe(style, probe, **{flag: True})
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(diagnostic, result.stderr)

    def test_optimized_interpreter_keeps_probe_assertions(self):
        for flag in (False, True):
            with self.subTest(missing_sync=flag):
                result = self.run_probe('captured', 'fsync', optimized=True, NO_APPEND_SYNC=flag)
                if flag:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('append did not fsync a file', result.stderr)
                else:
                    self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()

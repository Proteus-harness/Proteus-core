"""Synthetic command-identity checks; never launch an evaluated executable."""
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch

import adapters


class PreparedBinaryIdentity(unittest.TestCase):
    def prepare_command(self, filename, with_sibling=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            core = root / 'core'
            configs = core / 'configs'
            configs.mkdir(parents=True)
            profile = configs / 'fixture.toml'
            profile.write_text('[tools]\nenabled = []\n')
            case = root / 'case'
            case.mkdir()
            (case / 'prompt.txt').write_text('synthetic prompt')
            runtime = root / 'runtime'
            runtime.mkdir()
            binary = root / 'bin' / filename
            binary.parent.mkdir()
            binary.write_bytes(b'selected fixture bytes, not executable code')
            if with_sibling:
                self.assertNotEqual(filename, 'proteus')
                (binary.parent / 'proteus').write_bytes(b'unselected sibling bytes')
            args = SimpleNamespace(
                agent='proteus', binary=binary.resolve(), base_config=profile,
                case_dir=case, model='synthetic-model', reasoning='low', timeout=1,
                auth_file=root / 'unused-auth-placeholder',
            )
            selected = args.binary
            before = selected.read_bytes()
            # Only prepare() runs. Do not import native helpers or read auth.
            with patch.object(adapters, 'CORE', core), \
                    patch.object(adapters, 'load_module', return_value=SimpleNamespace()) as load, \
                    patch.object(adapters, 'copy_auth') as auth:
                command, _ = adapters.prepare(args, runtime)
            load.assert_called_once()
            auth.assert_called_once()
            self.assertEqual(args.binary, selected)
            self.assertEqual(selected.read_bytes(), before)
            self.assertEqual(command[1:], [
                '--config', '/runtime/configs/eval.json',
                '--cwd', '/workspace', 'synthetic prompt',
            ])
            return command

    def test_default_basename_preserves_command(self):
        self.assertEqual(self.prepare_command('proteus')[0], '/runtime/bin/proteus')

    def test_renamed_binary_does_not_select_existing_sibling_name(self):
        command = self.prepare_command('proteus-candidate', with_sibling=True)
        self.assertEqual(command[0], '/runtime/bin/proteus-candidate')

    def test_renamed_binary_does_not_require_default_sibling_name(self):
        command = self.prepare_command('candidate-only')
        self.assertEqual(command[0], '/runtime/bin/candidate-only')

    def test_spaces_and_unicode_remain_one_argv_token(self):
        command = self.prepare_command('proteus candidate 候補')
        self.assertEqual(command[0], '/runtime/bin/proteus candidate 候補')
        self.assertEqual(len(command), 6)


if __name__ == '__main__':
    unittest.main()

"""Real installer publication with synthetic module installers; no Cargo/npm/network."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
INSTALL = ROOT / 'install.sh'


def executable(path, body):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)
    path.chmod(0o755)


class InstallStagingTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='proteus install staging ')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.commands = self.root / 'commands'
        self.commands.mkdir()
        self.runtime = self.root / 'runtime'
        old = self.runtime / 'releases/previous'
        old.mkdir(parents=True)
        (old / 'sentinel').write_text('previous release')
        (self.runtime / 'current').symlink_to('releases/previous')
        self.target = self.root / 'target'
        executable(self.target / 'release/proteus', '#!/bin/sh\nexit 0\n')
        executable(self.commands / 'cargo', '#!/bin/sh\nexit 0\n')
        for name in ('node', 'npm'):
            executable(self.commands / name, '#!/bin/sh\nexit 0\n')
        modules = self.root / 'modules'
        modules.mkdir()
        (modules / 'Cargo.toml').write_text('# synthetic Modules source marker\n')
        (modules / 'package.py').write_text('''import argparse
from pathlib import Path
p=argparse.ArgumentParser()
p.add_argument('--profile')
p.add_argument('--bundle',type=Path)
a=p.parse_args()
b=a.bundle/'bin'
b.mkdir(parents=True)
f=b/'proteus-model-pack-module'
f.write_text('#!/bin/sh\\nexit 0\\n')
f.chmod(0o755)
''')
        runtime_installer = '''import argparse,json,os
from pathlib import Path
p=argparse.ArgumentParser()
p.add_argument('--prefix',type=Path,required=True)
a=p.parse_args()
name={name!r}
current=Path(os.environ['PROTEUS_HOME'])/'current'
assert current.resolve().name=='previous', 'published before runtime installation'
assert a.prefix.name.startswith('.') and a.prefix.name.endswith('.tmp')
with Path(os.environ['INSTALL_TRACE']).open('a') as trace:
 trace.write(json.dumps({{'name':name,'prefix':str(a.prefix)}})+'\\n')
if os.environ.get('FAIL_RUNTIME')==name:
 raise SystemExit(17)
f=a.prefix/'bin'/('proteus-'+name+'-module')
f.write_text('#!/bin/sh\\nexit 0\\n')
f.chmod(0o755)
worker=a.prefix/'share'/(name+'-runtime')/'worker.mjs'
worker.parent.mkdir(parents=True)
worker.write_text('// staged native worker fixture\\n')
'''
        for name, relative in (('pi', 'tool/pi-tools/install.py'),
                               ('opencode', 'workflow/opencode-runtime/install.py')):
            path = modules / relative
            path.parent.mkdir(parents=True)
            path.write_text(runtime_installer.format(name=name))
        self.trace = self.root / 'install-trace.jsonl'
        self.env = {**os.environ, 'PATH': str(self.commands) + os.pathsep + os.environ['PATH'],
                    'PROTEUS_BIN_DIR': str(self.root / 'bin'), 'PROTEUS_HOME': str(self.runtime),
                    'PROTEUS_CONFIG_HOME': str(self.root / 'config'),
                    'PROTEUS_MODULES_SOURCE': str(modules), 'CARGO_TARGET_DIR': str(self.target),
                    'INSTALL_TRACE': str(self.trace)}
        self.env.pop('FAIL_RUNTIME', None)

    def install(self):
        return subprocess.run(['/bin/sh', str(INSTALL)], env=self.env,
                              text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    def test_both_node_workers_are_ready_before_snapshot_publication(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        current = (self.runtime / 'current').resolve()
        self.assertNotEqual(current.name, 'previous')
        for name in ('pi', 'opencode'):
            self.assertTrue(os.access(current / 'bin' / ('proteus-' + name + '-module'), os.X_OK))
            self.assertTrue((current / 'share' / (name + '-runtime') / 'worker.mjs').is_file())
        trace = [json.loads(line) for line in self.trace.read_text().splitlines()]
        self.assertEqual([row['name'] for row in trace], ['pi', 'opencode'])
        self.assertEqual(trace[0]['prefix'], trace[1]['prefix'])
        self.assertFalse(Path(trace[0]['prefix']).exists())

    def test_node_install_failure_preserves_previous_release_and_removes_stage(self):
        self.env['FAIL_RUNTIME'] = 'opencode'
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.runtime / 'current').resolve().name, 'previous')
        self.assertEqual((self.runtime / 'current/sentinel').read_text(), 'previous release')
        self.assertEqual([path.name for path in (self.runtime / 'releases').iterdir()], ['previous'])
        self.assertFalse((self.root / 'bin/proteus').exists())
        trace = [json.loads(line) for line in self.trace.read_text().splitlines()]
        self.assertEqual([row['name'] for row in trace], ['pi', 'opencode'])

    def test_default_config_home_preserves_existing_named_profiles(self):
        self.env.pop('PROTEUS_CONFIG_HOME')
        home = self.root / 'home'
        self.env['HOME'] = str(home)
        configs = home / '.config/Proteus-harness/configs'
        configs.mkdir(parents=True)
        profile = configs / 'codex.config.toml'
        profile.write_text('# retained user configuration\n')
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(profile.read_text(), '# retained user configuration\n')
        self.assertTrue((configs / 'config.toml').is_file())
        self.assertTrue((configs / 'fragments/openai-chatgpt.toml').is_file())
        self.assertTrue((configs / 'fixtures/restricted.toml').is_file())
        for name in ('code-mode', 'extended-subscription', 'context-search-chatgpt', 'restricted'):
            self.assertFalse((configs / (name + '.config.toml')).exists(), name)
        self.assertFalse((home / '.config/Proteus-agent').exists())

    def test_missing_node_prerequisite_fails_before_build_or_staging(self):
        for missing in ('node', 'npm'):
            with self.subTest(missing=missing):
                commands = self.root / ('prerequisites-' + missing)
                commands.mkdir()
                (commands / 'dirname').symlink_to(shutil.which('dirname'))
                if missing == 'npm':
                    executable(commands / 'node', '#!/bin/sh\nexit 0\n')
                self.env['PATH'] = str(commands)
                result = self.install()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("required command '" + missing + "' not found", result.stderr)
                self.assertFalse(self.trace.exists())
                self.assertEqual((self.runtime / 'current').resolve().name, 'previous')
                self.assertEqual([path.name for path in (self.runtime / 'releases').iterdir()], ['previous'])


if __name__ == '__main__':
    unittest.main()

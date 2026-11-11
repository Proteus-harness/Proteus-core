"""Offline regressions for benchmark accounting and process containment."""
from contextlib import chdir
import json
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import adapters
import corpus
import runner


class RunnerChecks(unittest.TestCase):
    def test_sampler_grader_supports_standard_dataclass_import(self):
        grader = Path(__file__).with_name('long_repo_fixtures')/'sampler-phase-report/grader/test_outputs.py'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            module = root/'examples/performance/common/phase_report.py'
            module.parent.mkdir(parents=True)
            module.write_text('from __future__ import annotations\nfrom dataclasses import dataclass\n'
                              '@dataclass\nclass Row:\n    value: int\n'
                              'def summarize():\n    return Row(7)\n')
            try:
                # This regression exercises the real loader without requiring
                # host pytest; the grader's parameterized test bodies stay idle.
                pytest_stub = SimpleNamespace(mark=SimpleNamespace(
                    parametrize=lambda *args, **kwargs: lambda function: function))
                with patch.dict(sys.modules, {'pytest':pytest_stub}):
                    scope = runpy.run_path(str(grader))
                load = scope['load']
                load.__globals__['ROOT'] = root
                value = load()()
                self.assertEqual(value.value, 7)
                self.assertIs(sys.modules[value.__class__.__module__].Row, type(value))
            finally:
                sys.modules.pop('phase_under_test', None)

    def test_relative_workspace_preserves_protected_files_in_grader(self):
        with tempfile.TemporaryDirectory() as directory, chdir(directory):
            case, workspace = Path("case"), Path("workspace")
            (case/"grader").mkdir(parents=True)
            workspace.mkdir()
            protected = workspace/"release_test.py"
            protected.write_text("original")
            corpus.write_json(case/"case.json", {"id":"example", "grader_hashes":{},
                              "repo_hashes":{protected.name:corpus.digest(protected)}, "command":["true"]})
            with patch.object(corpus, "sandbox", return_value={"returncode":0}):
                self.assertTrue(corpus.grade(case, workspace, None)["passed"])
                protected.write_text("modified")
                self.assertFalse(corpus.grade(case, workspace, None)["passed"])

    def test_native_pi_input_excludes_cached_input(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            rows = [{'type':'message_end','message':{'role':'assistant','model':'fixture',
                     'usage':{'input':10,'cacheRead':100,'cacheWrite':0,'output':5}}},
                    {'type':'agent_end'}]
            (output/'stdout.jsonl').write_text('\n'.join(json.dumps(row) for row in rows))
            metrics = adapters.metrics('pi',output)
            self.assertEqual(metrics['provider_usage']['input_tokens'],110)
            self.assertEqual(metrics['provider_usage']['output_tokens'],5)
            self.assertEqual(metrics['model_requests'],1)

    def test_missing_codex_usage_is_unknown(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            (output/'stdout.jsonl').write_text('{"type":"turn.completed","usage":{"input_tokens":50}}\n')
            metrics = adapters.metrics('codex',output)
            self.assertIsNone(metrics['provider_usage']['cached_input_tokens'])
            self.assertIsNone(metrics['model_requests'])

    def test_pipeline_requires_observed_model_and_reasoning(self):
        report = {'agent':'codex','model':'fixture','reasoning':'high','returncode':0,
                  'timed_out':False,'metrics':{'settled':True,'errors':[],
                                             'models':[],'reasoning_efforts':[]}}
        runner.check_pipeline(report)
        self.assertFalse(report['pipeline_ok'])
        report['metrics'].update(models=['fixture'],reasoning_efforts=['high'])
        runner.check_pipeline(report)
        self.assertTrue(report['pipeline_ok'])
        report['metrics']['errors'] = [{'type':'error'}]
        runner.check_pipeline(report)
        self.assertFalse(report['pipeline_ok'])

    def test_only_openai_credentials_are_staged_and_removed(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = Path(directory)
            source = runtime/'source.json'
            destination = runtime/'home/.pi/agent/auth.json'
            source.write_text(json.dumps({'openai':{'type':'oauth','access':'synthetic','expires':123},
                                          'unrelated':{'key':'synthetic-private'}}))
            adapters.copy_openai_auth(source,destination,pi=True)
            self.assertEqual(set(json.loads(destination.read_text())),{'openai-codex'})
            native = runtime/'home/.local/share/opencode/auth.json'
            adapters.copy_openai_auth(destination,native)
            self.assertEqual(set(json.loads(native.read_text())),{'openai'})
            runner.remove_auth(runtime)
            self.assertFalse(destination.exists())
            self.assertFalse(native.exists())

    def test_followups_bind_same_session_and_preserve_initial_prompt(self):
        args = SimpleNamespace(agent='proteus', output=Path('/run'), timeout=1200)
        report = {}
        commands = []
        def execute(cmd, output, timeout, environment, *, append):
            commands.append((cmd[cmd.index('--')+1:], append))
            return {'returncode':0, 'timed_out':False, 'wall_seconds':1}
        with patch.object(runner, 'execute', side_effect=execute), patch.object(
                runner, 'observed_session', side_effect=[('session',1),('session',2)]):
            runner.run_turns(args, ['proteus','one'], [], {}, report, ['one','two'])
        self.assertEqual(commands, [(['proteus','one'],False), (['proteus','--resume-session','session','two'],True)])
        self.assertTrue(report['continuity_verified'])
        self.assertEqual([x['observed_user_turns'] for x in report['turns']], [1,2])

    def test_restarted_followup_session_is_a_pipeline_error(self):
        args = SimpleNamespace(agent='pi', output=Path('/run'), timeout=1200)
        with patch.object(runner, 'execute', return_value={'returncode':0,'timed_out':False,'wall_seconds':1}), patch.object(
                runner, 'observed_session', side_effect=[('old',1),('new',1)]):
            with self.assertRaisesRegex(ValueError, 'session identity'):
                runner.run_turns(args, ['pi','one'], [], {}, {}, ['one','two'])

    def test_outer_sandbox_can_only_write_disposable_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workspace, runtime = root/'workspace', root/'runtime'
            workspace.mkdir()
            runtime.mkdir()
            sessions = runtime/'sessions'
            sessions.mkdir()
            private_sentinel = root / "private-sentinel"
            private_sentinel.write_text("outside sandbox")
            command = runner.sandbox_command(['/usr/bin/python3','-c',
                "from pathlib import Path; import os; "
                f"assert not Path({str(private_sentinel)!r}).exists(); "
                "Path('/workspace/created').write_text('ok'); "
                "Path('/runtime/sessions/created').write_text('ok'); "
                "assert not os.access('/usr',os.W_OK)"], [(sessions,'/runtime/sessions',True)], workspace,runtime)
            subprocess.run(command,check=True)
            self.assertEqual((workspace/'created').read_text(),'ok')
            self.assertEqual((sessions/'created').read_text(),'ok')


if __name__ == '__main__':
    unittest.main()

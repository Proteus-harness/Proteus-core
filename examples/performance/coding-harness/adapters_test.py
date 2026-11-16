"""Offline checks for credential selection and disposable adapter routing."""
import json
from contextlib import closing
from pathlib import Path
import sqlite3
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import adapters
import open_code_native
import proteus_native
import runner

from open_code_native import active_export, prepare_auth_export, write_private_json


class AuthIsolation(unittest.TestCase):
    def test_export_stages_one_active_provider_without_other_credentials(self):
        records = [
            {'id': 'other', 'integrationID': 'unrelated', 'active': True, 'value': {'key': 'private-other'}},
            {'id': 'old', 'integrationID': 'opencode-go', 'active': False, 'value': {'key': 'private-old'}},
            {'id': 'selected', 'integrationID': 'opencode-go', 'active': True,
             'value': {'type': 'key', 'key': 'synthetic-selected'}},
        ]
        with tempfile.TemporaryDirectory() as directory:
            source, destination = Path(directory)/'source.json', Path(directory)/'selected.json'
            source.write_text(json.dumps(records))
            prepare_auth_export(source, destination, 'opencode-go')
            self.assertEqual(json.loads(destination.read_text()), [records[-1]])
            self.assertEqual(destination.stat().st_mode & 0o777, 0o600)

    def test_absent_and_ambiguous_active_credentials_fail_closed(self):
        active = {'integrationID': 'opencode-go', 'active': True}
        for records in [[], [{**active, 'active': False}], [active, active]]:
            with self.subTest(records=records), self.assertRaises(ValueError):
                active_export(records, 'opencode-go')

    def test_auth_write_cannot_replace_an_existing_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/'auth.json'
            write_private_json(path, {'synthetic': 'first'})
            with self.assertRaises(FileExistsError):
                write_private_json(path, {'synthetic': 'second'})
            self.assertEqual(json.loads(path.read_text()), {'synthetic': 'first'})


class FlashAuxiliaryGate(unittest.TestCase):
    def metadata(self, title):
        wanted = {'providerID': 'opencode-go', 'model': 'deepseek-v4.1-flash', 'variant': 'high'}
        return {
            'model.list': {'data': [{'id': wanted['model'], 'providerID': wanted['providerID'], 'variants': []}]},
            'plugin.list': [],
            'config.get': [{'info': {'model': wanted, 'agents': {'title': {'model': title}}}}],
        }

    def test_missing_or_different_title_cannot_generate(self):
        for title in [None, {'providerID':'opencode-go','model':'other-model','variant':'high'}]:
            with self.subTest(title=title), tempfile.TemporaryDirectory() as directory:
                metadata = self.metadata(title)
                with patch.object(open_code_native, 'api_json', side_effect=lambda b,e,u,op,**kw:metadata[op]):
                    with self.assertRaisesRegex(RuntimeError, 'refusing generation'):
                        open_code_native.save_registry('binary', {}, 'http://unused',
                            'opencode-go/deepseek-v4.1-flash#high', Path(directory))

    def test_exact_main_and_title_binding_is_retained_as_proof(self):
        metadata = self.metadata({'providerID':'opencode-go','model':'deepseek-v4.1-flash','variant':'high'})
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(open_code_native, 'api_json', side_effect=lambda b,e,u,op,**kw:metadata[op]):
                open_code_native.save_registry('binary', {}, 'http://unused',
                    'opencode-go/deepseek-v4.1-flash#high', Path(directory))
            proof = json.loads((Path(directory)/'registry.json').read_text())
            self.assertTrue(proof['auxiliary_model_verified'])
            self.assertEqual(proof['model_bindings']['main'], proof['model_bindings']['title'])


class NativeProteusAssembly(unittest.TestCase):
    def test_current_named_model_keeps_native_transport_and_profile_slots(self):
        fixtures = adapters.CORE/'target/performance/coding-harness/adapter-controls'
        fixtures.mkdir(parents=True, exist_ok=True)
        for disable in [False, True]:
            with self.subTest(disable=disable), tempfile.TemporaryDirectory(dir=fixtures) as directory:
                root = Path(directory)
                runtime, case = root/'runtime', root/'case'
                runtime.mkdir()
                case.mkdir()
                (case/'prompt.txt').write_text('synthetic offline control')
                args = SimpleNamespace(agent='proteus', model='opencode-go/deepseek-v4.1-flash',
                    reasoning='max', timeout=120, case_dir=case,
                    base_config=adapters.CORE/'configs/agent.toml',
                    binary=root/'portable/bin/candidate', disable_subagents=disable)
                export = [{'id': 'selected', 'integrationID': 'opencode-go', 'active': True,
                           'value': {'type':'key', 'key':'synthetic-control-key'}}]
                native = SimpleNamespace(fresh_go_export=lambda: export)
                with patch.object(adapters, 'load_module', return_value=native):
                    command, mounts = adapters.prepare(args, runtime)
                config = json.loads((runtime/'configs/eval.json').read_text())
                source, _ = proteus_native.selected_model(args.base_config)
                self.assertEqual(config['modules'], source['modules'])
                self.assertEqual(config['instructions'], source['instructions'])
                self.assertEqual(config['components'], source['components'])
                self.assertEqual(config['models']['cheap']['provider'], 'opencode')
                self.assertEqual(config['models']['cheap']['effort'], 'max')
                self.assertEqual(config['module_config']['model']['opencode']['provider'], 'opencode-go')
                self.assertNotIn('pi_codex', config['module_config']['model'])
                self.assertEqual(config['tools']['enabled'], source['tools']['enabled'])
                self.assertEqual(config['module_config']['policy']['codex_policy'], {
                    'allow': source['tools']['enabled'], 'allow_sandboxed': [], 'ask_before': []})
                if disable:
                    self.assertEqual(config['agent_control'], {'surface':'none', 'roles':[]})
                else:
                    self.assertEqual(config['agent_control'], source['agent_control'])
                self.assertEqual(command[:5], ['/runtime/bin/candidate', '--config',
                    '/runtime/configs/eval.json', '--cwd', '/workspace'])
                destinations = [mount[1] for mount in mounts]
                self.assertIn('/runtime/share/opencode-runtime', destinations)
                self.assertNotIn('/runtime/share/pi-runtime', destinations)
                database = runtime/'home/.local/share/opencode/opencode.db'
                self.assertEqual(database.stat().st_mode & 0o777, 0o600)
                with closing(sqlite3.connect(database)) as db:
                    rows = db.execute('SELECT integration_id,value FROM credential').fetchall()
                self.assertEqual(rows, [('opencode-go', json.dumps(export[0]['value']))])
                selection = proteus_native.bound_provider(runtime, [json.dumps({
                    'provider':'opencode', 'model':'deepseek-v4.1-flash'})])
                self.assertEqual(selection['provider'], 'opencode-go')
                with self.assertRaisesRegex(ValueError, 'Observed model adapter differs'):
                    proteus_native.bound_provider(runtime, [json.dumps({'provider':'pi_codex'})])
                (runtime/'sessions/fixture').mkdir()
                (runtime/'sessions/fixture/journal.jsonl').touch()
                canonical = {'model_refs': [json.dumps({'provider':'opencode',
                                 'model':'deepseek-v4.1-flash'})],
                             'reasoning_refs': [json.dumps({'effort':'max'})],
                             'model_requests':1, 'model_responses':1,
                             'settlements':[{'status':'success','error':None}]}
                usage = SimpleNamespace(measure=lambda path: canonical.copy(),
                    _journal=SimpleNamespace(records=lambda path: []))
                with patch.object(adapters, 'load_module', return_value=usage):
                    metrics = adapters.metrics('proteus', root)
                # metrics() expects output/runtime; the same prepared receipt
                # proves provider mapping even though native responses omit ID.
                self.assertEqual(metrics['providers'], ['opencode-go'])
                outcome = {'agent':'proteus', 'model':args.model, 'reasoning':args.reasoning,
                           'returncode':0, 'timed_out':False, 'metrics':metrics}
                runner.check_pipeline(outcome)
                self.assertTrue(outcome['pipeline_ok'])
                # Evidence must never contain private values; cleanup also runs
                # on prepare-only and exceptions, before writing result.json.
                for receipt in ['model-selection.json', 'profile-overrides.json']:
                    self.assertNotIn('synthetic-control-key', (runtime/receipt).read_text())
                runner.remove_auth(runtime)
                self.assertFalse(database.exists())
                config['models']['cheap']['effort'] = 'low'
                (runtime/'configs/eval.json').write_text(json.dumps(config))
                with self.assertRaisesRegex(ValueError, 'configuration changed'):
                    proteus_native.bound_provider(runtime, [json.dumps({'provider':'opencode'})])


if __name__ == '__main__':
    unittest.main()

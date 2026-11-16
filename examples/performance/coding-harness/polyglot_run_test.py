"""Budget accounting and admission boundaries for paid benchmark execution."""
import json
import contextlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

import polyglot_run as run

PRICES = {'input': .15, 'cache_read': .003, 'output': .6}


class PaidAdmissionTests(unittest.TestCase):
    def test_scheduler_stops_new_admission_after_an_unmetered_trial(self):
        with tempfile.TemporaryDirectory(dir=run.DATA) as temporary:
            root = Path(temporary)
            source = root/'cases'
            source.mkdir()
            (source/'manifest.json').write_text('{}')
            config, binary = root/'config.toml', root/'binary'
            config.write_text('')
            binary.write_bytes(b'fixture')
            tools = root/'toolchains'
            tools.mkdir()
            (tools/'receipt.json').write_text('{}')
            rows = [{'id': 'python--' + name, 'exercise': name, 'language': 'python'}
                    for name in ('first', 'second', 'third')]
            run.write_json(root/'run.json', {
                'agents': list(run.AGENTS), 'cases': rows,
                'case_order': [row['id'] for row in rows], 'case_root': str(source),
                'source_manifest_sha256': run.corpus.digest(source/'manifest.json'),
                'base_config': str(config), 'base_config_sha256': run.corpus.digest(config),
                'binaries': {agent: {'path': str(binary), 'sha256': run.corpus.digest(binary)}
                             for agent in run.AGENTS},
                'proteus_runtime': {}, 'conditions': {'soft_budget_usd': 20},
                'toolchain_setup_sha256': run.corpus.digest(tools/'receipt.json'),
            })
            def unknown_trial(_root, _frozen, agent, row):
                return {'agent': agent, 'case': row['id'], 'usage_complete': False,
                        'observed_cost_usd_lower_bound': 0, 'first_pass': False,
                        'passed': False, 'failure_kind': 'harness_pipeline_failure', 'wall_seconds': 1}
            with patch.object(run, 'runtime_identity', return_value={}), \
                 patch.object(run.grader, 'TOOLS', tools), \
                 patch.object(run, 'control_receipt', return_value={}), \
                 patch.object(run, 'run_case', side_effect=unknown_trial) as invoke, \
                 contextlib.redirect_stdout(io.StringIO()):
                run.run(SimpleNamespace(output=root, only=None, jobs=1))
            self.assertEqual(invoke.call_count, 1)
            self.assertEqual(len(run.read_json(root/'progress.json')['admission_blocked_unmetered']), 1)

    def test_inclusive_usage_does_not_charge_reasoning_twice(self):
        metrics = {'provider_usage': {'input_tokens': 1000, 'cached_input_tokens': 900,
                                     'output_tokens': 100, 'reasoning_output_tokens': 80}}
        estimate, lower, complete = run.cost_projection(metrics, PRICES)
        self.assertAlmostEqual(estimate, .0000777)
        self.assertEqual(estimate, lower)
        self.assertTrue(complete)

    def test_incomplete_usage_is_unknown_with_observed_lower_bound(self):
        metrics = {'provider_usage': {'input_tokens': None},
                   'partial_reported_usage_sums': {'cached_input_tokens': 50, 'output_tokens': 10}}
        estimate, lower, complete = run.cost_projection(metrics, PRICES)
        self.assertIsNone(estimate)
        self.assertAlmostEqual(lower, .00000615)
        self.assertFalse(complete)

    def test_provider_failure_classification_is_symmetric(self):
        evidence = [
            {'failures': [{'kind': 'model', 'failure': {'message': 'HTTP 503'}}]},
            {'errors': [{'error': {'data': {'statusCode': 503, 'message': 'Unavailable'}}}]},
        ]
        for metrics in evidence:
            self.assertEqual(run.classify_failure({'metrics': metrics}),
                             ('infrastructure_provider_transport_or_quota', True))
        self.assertEqual(run.classify_failure({'metrics': {
            'failures': [{'kind': 'tool', 'error': 'HTTP 503'}]}}),
            ('harness_pipeline_failure', False))

    def test_deadline_remains_a_budget_failure(self):
        self.assertEqual(run.classify_failure({'timed_out': True, 'metrics': {
            'errors': [{'statusCode': 503}]}}), ('agent_deadline', False))

    def test_controls_allow_declared_refactor_but_reject_changed_starter(self):
        with tempfile.TemporaryDirectory(dir=run.DATA) as temporary:
            base = Path(temporary)
            case = base/'cases/python--example'
            (case/'repo').mkdir(parents=True)
            (case/'repo/code.py').write_text('pass\n')
            (case/'case.json').write_text(json.dumps({'repo_hashes': run.corpus.manifest(case/'repo')}))
            (base/'controls').mkdir()
            receipt = {'case_sha256': run.corpus.digest(case/'case.json'),
                       'evaluable': True, 'starter_pass_allowed': True,
                       'starter': {'passed': True}, 'reference': {'passed': True}}
            (base/'controls/python--example.json').write_text(json.dumps(receipt))
            with patch.object(run, 'DATA', base):
                self.assertEqual(run.control_receipt(case), receipt)
                (case/'repo/code.py').write_text('changed\n')
                with self.assertRaisesRegex(ValueError, 'Starter tree'):
                    run.control_receipt(case)

    def test_toolchain_environment_reaches_only_shell_component(self):
        with tempfile.TemporaryDirectory(dir=run.DATA) as temporary:
            root = Path(temporary)
            (root/'configs').mkdir()
            config = {'components': {
                'shell': {'env_allowlist': ['HOME'], 'exports': {'tool': {'shell_tools': {}}}},
                'model': {'env_allowlist': ['HOME'], 'exports': {'model': {'opencode': {}}}},
            }}
            run.write_json(root/'configs/eval.json', config)
            run.write_json(root/'model-selection.json', {})
            run.write_json(root/'profile-overrides.json', {})
            run.toolchain_environment_override(root, {'HOME': '/state/home', 'PATH': '/runtime/bin',
                                                       'JAVA_HOME': '/polyglot/jdk',
                                                       'GRADLE_RO_DEP_CACHE': '/polyglot/deps'})
            result = run.read_json(root/'configs/eval.json')
            self.assertIn('GRADLE_RO_DEP_CACHE', result['components']['shell']['env_allowlist'])
            self.assertEqual(result['components']['model']['env_allowlist'], ['HOME', 'NODE_USE_ENV_PROXY'])
            self.assertNotIn('GRADLE_RO_DEP_CACHE', result['components']['model']['env_allowlist'])
            self.assertEqual(run.read_json(root/'model-selection.json')['effective_config_sha256'],
                             run.corpus.digest(root/'configs/eval.json'))


if __name__ == '__main__':
    unittest.main()

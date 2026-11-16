"""Synthetic evidence controls for cost accounting and clustered comparisons."""
import csv
import json
from pathlib import Path
import tempfile
import unittest

import polyglot_report as report

CORE = Path(__file__).resolve().parents[3]


class PolyglotReports(unittest.TestCase):
    def setUp(self):
        fixtures = CORE/'target/performance/polyglot-report-controls'
        fixtures.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(dir=fixtures)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def manifest(self, cases):
        (self.root/'run.json').write_text(json.dumps({'cases': cases, 'agents': ['proteus', 'opencode'],
                                                    'conditions': {'effort': 'high'}}))

    def trial(self, case, agent, *, passed=True, pipeline=True, complete=True,
              cost=1, lower=1, failure=None, usage=None):
        path = self.root/'trials'/agent/case['id']
        path.mkdir(parents=True)
        result = {'agent': agent, 'case': case['id'], 'pipeline_ok': pipeline,
                  'first_pass': passed, 'passed': passed, 'grades': [{'passed': passed}], 'usage_complete': complete,
                  'usage_cost_usd': cost, 'observed_cost_usd_lower_bound': lower,
                  'failure_kind': failure, 'wall_seconds': 5, 'trial_wall_seconds': 8,
                  'auth_removed': True,
                  # Both turns are cumulative snapshots. They must not be
                  # added to the final metrics a second time.
                  'turns': [{'metrics': {'provider_usage': {'input_tokens': 30}}},
                            {'metrics': {'provider_usage': {'input_tokens': 100}}}],
                  'metrics': {'provider_usage': usage or {'input_tokens': 100,
                                                        'cached_input_tokens': 80,
                                                        'output_tokens': 10},
                              'all_tool_calls': 3, 'model_requests': 2}}
        (path/'result.json').write_text(json.dumps(result))

    def test_missing_jobs_do_not_become_model_failures_and_cumulative_usage_counted_once(self):
        cases = [{'id': name, 'language': 'python', 'exercise': name} for name in ('a', 'b', 'c')]
        self.manifest(cases)
        self.trial(cases[0], 'proteus')
        self.trial(cases[1], 'proteus', passed=False, cost=3, lower=3, failure='agent_deadline')
        self.trial(cases[0], 'opencode')
        self.trial(cases[1], 'opencode')
        result = report.build_report(self.root, repetitions=200)
        proteus = result['agents']['proteus']
        self.assertEqual((proteus['attempted'], proteus['unrun']), (2, 1))
        self.assertIsNone(proteus['full_benchmark_score'])
        self.assertEqual(proteus['final_rate_observed'], .5)
        self.assertEqual(proteus['tokens']['input_tokens']['total'], 200)
        self.assertEqual(proteus['cost_per_clean_success_usd'], 4)
        paired = result['paired']['all_completed_pairs']['final']
        self.assertEqual(paired['cases'], 2)
        self.assertEqual(paired['delta']['rate'], -.5)
        self.assertEqual(result['paired']['without_explicit_infrastructure']['final']['cases'], 2)
        self.assertIn('отсутствующие результаты не считаются ошибками модели', report.markdown(result))
        output = self.root/'results.csv'
        report.write_csv(output, result['rows'])
        with output.open() as stream:
            rows = list(csv.DictReader(stream))
        missing = next(row for row in rows if row['case'] == 'c')
        self.assertEqual(missing['status'], 'unrun')
        self.assertEqual(missing['passed'], '')
        self.assertEqual(missing['input_tokens'], '')

    def test_partial_usage_is_unknown_with_separate_observed_lower_bound(self):
        cases = [{'id': name, 'language': 'python', 'exercise': name} for name in ('a', 'b')]
        self.manifest(cases)
        self.trial(cases[0], 'proteus')
        self.trial(cases[1], 'proteus', passed=False, complete=False, cost=None, lower=.4,
                   usage={'input_tokens': 200, 'cached_input_tokens': 100, 'output_tokens': 20})
        result = report.build_report(self.root, repetitions=200)['agents']['proteus']
        self.assertIsNone(result['tokens']['input_tokens']['total'])
        self.assertEqual(result['tokens']['input_tokens']['observed_lower_bound'], 300)
        self.assertIsNone(result['tokens']['reasoning_output_tokens']['observed_lower_bound'])
        self.assertIsNone(result['cost_usd']['total'])
        self.assertAlmostEqual(result['cost_usd']['observed_lower_bound'], 1.4)
        self.assertIsNone(result['cost_per_clean_success_usd'])
        self.assertEqual(result['cache_hit']['known_trials'], 1)
        self.assertFalse(result['cache_hit']['covers_all_attempted'])

    def test_grader_pass_with_failed_pipeline_is_not_success_and_infra_is_explicit(self):
        cases = [{'id': name, 'language': 'go', 'exercise': name} for name in ('a', 'b', 'c')]
        self.manifest(cases)
        for case in cases:
            self.trial(case, 'opencode')
        self.trial(cases[0], 'proteus', pipeline=False, failure='infrastructure_transport')
        self.trial(cases[1], 'proteus', passed=False, failure='agent_deadline')
        self.trial(cases[2], 'proteus')
        result = report.build_report(self.root, repetitions=200)
        summary = result['agents']['proteus']
        self.assertEqual(summary['grader_passes'], 2)
        self.assertEqual(summary['clean_successes'], 1)
        self.assertEqual(summary['infrastructure_failures'], 1)
        self.assertEqual(result['paired']['all_completed_pairs']['final']['cases'], 3)
        clean = result['paired']['without_explicit_infrastructure']
        self.assertEqual(clean['final']['cases'], 2)
        self.assertEqual(clean['excluded_pairs'], 1)
        self.assertEqual(clean['final']['arms']['proteus']['rate'], .5)

    def test_grader_pass_uses_independent_last_grade_even_if_pipeline_failed(self):
        case = {'id': 'a', 'language': 'python', 'exercise': 'a'}
        self.manifest([case])
        self.trial(case, 'proteus', passed=False, pipeline=False)
        path = self.root/'trials/proteus/a/result.json'
        result = json.loads(path.read_text())
        result['grades'] = [{'passed': False}, {'passed': True}]
        path.write_text(json.dumps(result))
        summary = report.build_report(self.root, repetitions=100)['agents']['proteus']
        self.assertEqual(summary['grader_passes'], 1)
        self.assertEqual(summary['clean_successes'], 0)

    def test_zero_test_case_is_explicit_corpus_exclusion_and_baseline_pass_has_secondary_slice(self):
        cases = [
            {'id': 'go--counter', 'language': 'go', 'exercise': 'counter', 'evaluable': False,
             'evaluation_reason': 'Upstream has zero tests', 'baseline_pass': True},
            {'id': 'go--ledger', 'language': 'go', 'exercise': 'ledger', 'evaluable': True,
             'baseline_pass': True},
            {'id': 'python--grep', 'language': 'python', 'exercise': 'grep', 'evaluable': True,
             'baseline_pass': False},
        ]
        self.manifest(cases)
        for agent in ('proteus', 'opencode'):
            self.trial(cases[1], agent)
            self.trial(cases[2], agent, passed=False)
        result = report.build_report(self.root, repetitions=100)
        self.assertEqual(result['corpus_cases'], 3)
        self.assertEqual(result['planned_cases'], 2)
        self.assertEqual(len(result['excluded_cases']), 1)
        self.assertTrue(result['complete'])
        summary = result['agents']['proteus']
        self.assertEqual((summary['planned'], summary['unrun'], summary['excluded']), (2, 0, 1))
        self.assertEqual(summary['full_benchmark_score'], .5)
        self.assertEqual(result['paired']['all_completed_pairs']['final']['cases'], 2)
        self.assertEqual(result['nontrivial']['planned_cases'], 1)
        self.assertEqual(result['nontrivial']['agents']['proteus']['clean_successes'], 0)
        excluded = [row for row in result['rows'] if not row['evaluable']]
        self.assertEqual([row['status'] for row in excluded], ['excluded', 'excluded'])
        self.assertIn('zero tests', report.markdown(result))

    def test_bootstrap_keeps_language_variants_and_both_arms_in_same_cluster(self):
        pairs = []
        for exercise, proteus_pass in [('a', True), ('b', False)]:
            for language in ('python', 'rust', 'javascript'):
                pairs.append({agent: {'exercise': exercise, 'language': language,
                                      'passed': proteus_pass if agent == 'proteus' else not proteus_pass}
                              for agent in ('proteus', 'opencode')})
        first = report.cluster_interval(pairs, ['proteus', 'opencode'], 'passed', 500, 7)
        second = report.cluster_interval(pairs, ['proteus', 'opencode'], 'passed', 500, 7)
        self.assertEqual(first, second)
        self.assertEqual(first['exercise_clusters'], 2)
        self.assertEqual(first['cases'], 6)
        self.assertEqual(first['delta']['ci95'], [-1, 1])
        for pair in pairs:
            pair['opencode']['passed'] = pair['proteus']['passed']
        shared = report.cluster_interval(pairs, ['proteus', 'opencode'], 'passed', 500, 7)
        self.assertEqual(shared['delta']['ci95'], [0, 0])


if __name__ == '__main__':
    unittest.main()

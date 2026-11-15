"""Synthetic artifact controls for shared short/long usage projection."""
import json
from pathlib import Path
import tempfile
import unittest

import long_report
import report
import usage_report


class UsageReportChecks(unittest.TestCase):
    def trial(self, root, case, attempt=1, *, pipeline=True, grade=True,
              timed_out=False, tokens=None, partial=None, events=()):
        path = root/f'fixture_{case}_{attempt}_pi'
        path.mkdir()
        metrics = {'provider_usage': tokens or {}, 'model_requests': 1,
                   'all_tool_calls': 2, 'tool_names': {'read_file': 2}, 'errors': []}
        if partial is not None:
            metrics['partial_reported_usage_sums'] = partial
        result = {'agent': 'pi', 'model': 'fixture', 'reasoning': 'high',
                  'pipeline_ok': pipeline, 'timed_out': timed_out,
                  'returncode': 124 if timed_out else 0, 'expected_turns': 1,
                  'wall_seconds': 1.0, 'prompt_sha256': 'prompt',
                  'fixture_files_sha256': {'fixture.txt': 'contents'}, 'metrics': metrics}
        (path/'result.json').write_text(json.dumps(result))
        (path/'grading.json').write_text(json.dumps({'passed': grade}))
        (path/'stdout.jsonl').write_text(''.join(json.dumps(event)+'\n' for event in events))
        return path, result

    def test_short_and_long_keep_same_complete_and_partial_row_evidence(self):
        tokens = {'input_tokens': 10, 'cached_input_tokens': 2, 'output_tokens': 3}
        for complete in [True, False]:
            with self.subTest(complete=complete), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                path, _ = self.trial(root, 'case', pipeline=complete, timed_out=not complete,
                                     tokens=tokens)
                original = {file.name: file.read_bytes() for file in path.iterdir()}
                short = report.read_rows([root], [])[0]
                long = long_report.read_trial(path, 'case', 'pi', 1)
                for key in ['tokens', 'reported_usage', 'observed_partial_usage',
                            'usage_complete', 'usage_completeness_reason', 'timed_out']:
                    self.assertEqual(short[key], long[key], key)
                self.assertEqual(short['usage_complete'], complete)
                self.assertEqual(short['tokens']['input_tokens'], 10 if complete else None)
                self.assertEqual(short['reported_usage']['input_tokens'], 10)
                self.assertEqual(short['observed_partial_usage']['input_tokens'], 10)
                self.assertEqual(original, {file.name: file.read_bytes() for file in path.iterdir()})

    def test_partial_primary_is_not_promoted_and_repeat_does_not_enter_totals(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first, _ = self.trial(root, 'first', grade=False,
                                  tokens={'input_tokens': 10, 'cached_input_tokens': 2, 'output_tokens': 3})
            second, _ = self.trial(root, 'second', pipeline=False, timed_out=True,
                                   tokens={'input_tokens': 20, 'cached_input_tokens': 4, 'output_tokens': 6})
            repeat, _ = self.trial(root, 'second', 2,
                                   tokens={'input_tokens': 1000, 'cached_input_tokens': 200, 'output_tokens': 300})
            short_rows = report.read_rows([root], [])
            short = report.summarize(short_rows)[0]
            long_rows = [long_report.read_trial(first, 'first', 'pi', 1),
                         long_report.read_trial(second, 'second', 'pi', 1),
                         long_report.read_trial(repeat, 'second', 'pi', 2)]
            long = long_report.summarize(long_rows, ['pi'], ['first', 'second'])[0]
            self.assertEqual(short['primary_attempts'], 2)
            self.assertEqual(short['primary_passed'], 0)
            self.assertEqual(short['repeat_attempts'], 1)
            self.assertEqual(short['repeat_passed'], 1)
            self.assertEqual(short['requests'], 2)
            self.assertEqual(short['actions'], 4)
            for field in ['input', 'cached', 'output']:
                self.assertIsNone(short[field])
            self.assertEqual(short['known_token_runs'], long['known_token_runs'])
            self.assertEqual(short['observed_token_sums'], long['observed_token_sums'])
            self.assertEqual(short['known_token_runs']['input_tokens'], 1)
            self.assertEqual(short['observed_token_sums']['input_tokens'], 30)
            self.assertEqual(short['observed_token_sums']['output_tokens'], 9)
            self.assertIsNone(long['tokens']['input_tokens'])

    def test_complete_run_with_missing_category_keeps_observed_partial_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.trial(root, 'case', tokens={'input_tokens': None, 'output_tokens': 3},
                       partial={'input_tokens': 7})
            row = report.read_rows([root], [])[0]
            summary = report.summarize([row])[0]
            self.assertTrue(row['usage_complete'])
            self.assertIsNone(summary['input'])
            self.assertEqual(summary['output'], 3)
            self.assertEqual(summary['known_token_runs']['input_tokens'], 0)
            self.assertEqual(summary['observed_token_sums']['input_tokens'], 7)
            self.assertEqual(row['reported_usage']['input_tokens'], None)

    def test_explicit_zero_and_no_observation_have_distinct_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            zero = usage_report.project_usage(path, {'pipeline_ok': True},
                                               {'provider_usage': {'input_tokens': 0}})
            empty = usage_report.project_usage(path, {'pipeline_ok': True}, {})
            zeros = usage_report.summarize_usage([zero])
            absent = usage_report.summarize_usage([empty])
            self.assertEqual(zeros['tokens']['input_tokens'], 0)
            self.assertEqual(zeros['known_token_runs']['input_tokens'], 1)
            self.assertIsNone(absent['tokens']['input_tokens'])
            self.assertEqual(absent['known_token_runs']['input_tokens'], 0)
            self.assertEqual(zeros['observed_token_sums']['input_tokens'], 0)
            self.assertEqual(absent['observed_token_sums']['input_tokens'], 0)
            zero['tokens']['input_tokens'] = 9
            self.assertEqual(zero['reported_usage']['input_tokens'], 0)

    def test_existing_teardown_completion_policy_is_not_replaced_by_pipeline_boolean(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            events = [{'type': 'agent_start'},
                      {'type': 'agent_end', 'willRetry': False,
                       'messages': [{'role': 'assistant', 'stopReason': 'stop'}]},
                      {'type': 'agent_settled', 'aborted': False}]
            (path/'stdout.jsonl').write_text(''.join(json.dumps(event)+'\n' for event in events))
            complete = usage_report.project_usage(path,
                {'agent': 'pi', 'pipeline_ok': False, 'expected_turns': 1},
                {'provider_usage': {'input_tokens': 10}})
            self.assertTrue(complete['usage_complete'])
            self.assertEqual(complete['tokens']['input_tokens'], 10)
            canonical = usage_report.project_usage(path,
                {'agent': 'proteus', 'pipeline_ok': False},
                {'model_requests': 1, 'model_responses': 1,
                 'settlements': [{'status': 'success'}], 'failures': [],
                 'provider_usage': {'input_tokens': 12}})
            self.assertTrue(canonical['usage_complete'])
            self.assertEqual(canonical['tokens']['input_tokens'], 12)

    def test_pair_identity_guard_still_rejects_different_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.trial(root, 'case', tokens={'input_tokens': 1})
            second, result = self.trial(root, 'case', 2, tokens={'input_tokens': 1})
            result['prompt_sha256'] = 'different'
            (second/'result.json').write_text(json.dumps(result))
            with self.assertRaisesRegex(ValueError, 'fixture or prompt differs'):
                report.summarize(report.read_rows([root], []))


if __name__ == '__main__':
    unittest.main()

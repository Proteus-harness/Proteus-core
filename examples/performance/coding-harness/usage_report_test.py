"""Synthetic artifact controls for shared short/long usage projection."""
import json
from pathlib import Path
import tempfile
import shutil
from unittest.mock import patch
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

    def test_short_identity_uses_actual_configuration_not_directory_alias(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first, result = self.trial(root, 'case', tokens={'input_tokens': 10})
            alias = root/'alias_case_1_pi'
            shutil.copytree(first, alias)
            with self.assertRaisesRegex(ValueError, 'duplicate run'):
                report.read_rows([root], [])
            numeric_alias = root/'fixture_case_01_pi'
            alias.rename(numeric_alias)
            with self.assertRaisesRegex(ValueError, 'duplicate run'):
                report.read_rows([root], [])
            numeric_alias.rename(alias)
            result['reasoning'] = 'low'
            (alias/'result.json').write_text(json.dumps(result))
            self.assertEqual(len(report.summarize(report.read_rows([root], []))), 2)
            result['reasoning'] = 'high'
            result['model'] = 'other-real-model'
            (alias/'result.json').write_text(json.dumps(result))
            self.assertEqual(len(report.summarize(report.read_rows([root], []))), 2)

    def test_repeat_only_short_summary_has_unknown_primary_median(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.trial(root, 'case', 2, tokens={'input_tokens': 10})
            summary = report.summarize(report.read_rows([root], []))[0]
            self.assertEqual(summary['primary_attempts'], 0)
            self.assertEqual(summary['repeat_attempts'], 1)
            self.assertIsNone(summary['median_wall_seconds'])
            self.assertIsNone(summary['input'])

    def test_long_plan_rejects_duplicates_before_reading(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for cases, arms, repeats in [(['x', 'x'], ['pi'], []),
                                          (['x'], ['pi', 'pi'], []),
                                          (['x'], ['pi'], ['x', 'x'])]:
                with self.subTest(cases=cases, arms=arms, repeats=repeats), \
                     patch.object(long_report, 'read_trial', side_effect=AssertionError('must not read')):
                    with self.assertRaisesRegex(ValueError, 'duplicate'):
                        long_report.build(root, [], cases, arms, root/'absent', repeats)

    def test_long_selected_axes_and_fixed_study_configuration(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path, result = self.trial(root, 'custom', tokens={'input_tokens': 10})
            selected = root/'flash_custom_1_pi'
            path.rename(selected)
            result['model'] = 'opencode-go/deepseek-v4.1-flash'
            (selected/'result.json').write_text(json.dumps(result))
            with patch.object(long_report, 'DATA', root/'empty-history'):
                document = long_report.build(root, [], ['custom'], ['pi'], root/'absent', [])
                self.assertTrue(document['complete'])
                self.assertEqual(document['cases'], ['custom'])
                self.assertEqual(document['arms'], ['pi'])
                output = root/'report.md'
                long_report.write_markdown(document, output)
                self.assertIn('| Задача | Pi |', output.read_text())
                self.assertIn('| custom | PASS', output.read_text())
                self.assertNotIn('| db-wal-recovery |', output.read_text())
                shutil.copytree(selected, root/'flash_custom_2_pi')
                repeated = long_report.build(root, [], ['custom'], ['pi'], root/'absent', ['custom'])
                long_report.write_markdown(repeated, output)
                self.assertIn('| Задача | Агент | Полный PASS сначала / повтор |', output.read_text())
                self.assertIn('| custom | pi | True / True |', output.read_text())
                self.assertNotIn('повтор go-review', output.read_text())
                for field, value in [('model', 'wrong-model'), ('reasoning', 'low')]:
                    changed = {**result, field: value}
                    (selected/'result.json').write_text(json.dumps(changed))
                    with self.assertRaisesRegex(ValueError, 'model or effort'):
                        long_report.build(root, [], ['custom'], ['pi'], root/'absent', [])
                (selected/'result.json').unlink()
                pending = long_report.build(root, [], ['custom'], ['pi'], root/'absent', [])
                self.assertFalse(pending['complete'])

    def test_long_diagnostics_share_study_but_historical_baseline_is_separate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runs = root/'runs'
            runs.mkdir()
            path, result = self.trial(runs, 'custom', tokens={'input_tokens': 10})
            result['model'] = 'opencode-go/deepseek-v4.1-flash'
            (path/'result.json').write_text(json.dumps(result))
            path.rename(runs/'flash_custom_1_pi')
            baseline = root/'baseline.json'
            baseline.write_text(json.dumps({'rows': [{'attempt': 1, 'arm': 'pi',
                'model': 'historical-other-model', 'effort': 'low'}]}))
            for kind in ('cap', 'retry'):
                folder = root/kind
                folder.mkdir()
                trial = folder/'flash_custom_1_pi'
                shutil.copytree(runs/'flash_custom_1_pi', trial)
                if kind == 'retry':
                    (folder/'selection.json').write_text(json.dumps({'rows': [['flash', 'custom', 1, 'pi']]}))
                options = {'control_root' if kind == 'cap' else 'retry_root': folder}
                with patch.object(long_report, 'DATA', root/'empty-history'):
                    accepted = long_report.build(runs, [], ['custom'], ['pi'], baseline, [], **options)
                    self.assertTrue(accepted['complete'])
                    if kind == 'retry':
                        output = root/'retry.md'
                        long_report.write_markdown(accepted, output)
                        self.assertIn('Число выбранных клеток для свежего повтора: 1.', output.read_text())
                        self.assertNotIn('заранее выбранных пяти клеток', output.read_text())
                    for field, value in [('model', 'other'), ('reasoning', 'low')]:
                        (trial/'result.json').write_text(json.dumps({**result, field: value}))
                        with self.assertRaisesRegex(ValueError, 'model or effort'):
                            long_report.build(runs, [], ['custom'], ['pi'], baseline, [], **options)

    def test_long_diagnostic_identity_is_unique_within_each_cohort(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runs = root/'runs'
            runs.mkdir()
            path, result = self.trial(runs, 'custom', tokens={'input_tokens': 10})
            result['model'] = 'opencode-go/deepseek-v4.1-flash'
            (path/'result.json').write_text(json.dumps(result))
            path.rename(runs/'flash_custom_1_pi')
            cap, retry = root/'cap', root/'retry'
            cap.mkdir()
            retry.mkdir()
            for folder in (cap, retry):
                shutil.copytree(runs/'flash_custom_1_pi', folder/'flash_custom_1_pi')
            selection = retry/'selection.json'
            selected = [['flash', 'custom', 1, 'pi']]
            selection.write_text(json.dumps({'rows': selected}))
            with patch.object(long_report, 'DATA', root/'empty-history'):
                def build():
                    return long_report.build(runs, [], ['custom'], ['pi'], root/'absent', [], cap, retry)
                self.assertTrue(build()['complete'])  # Same identity across cohorts is intentional.
                selection.write_text(json.dumps({'rows': selected * 2}))
                with self.assertRaisesRegex(ValueError, 'duplicate trial in retry cohort'):
                    build()
                selection.write_text(json.dumps({'rows': selected}))
                shutil.copytree(cap/'flash_custom_1_pi', cap/'flash_custom_01_pi')
                with self.assertRaisesRegex(ValueError, 'duplicate trial in cap cohort'):
                    build()

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

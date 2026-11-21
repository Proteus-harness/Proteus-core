"""Synthetic completed-event accounting; no native CLI or model invocation."""
import json
from pathlib import Path
import tempfile
import unittest

import adapters
import long_report
import report
import usage_report

MISSING = object()
PI = {'input': 10, 'cacheRead': 100, 'cacheWrite': 0, 'output': 5}
CODEX = {'input_tokens': 110, 'cached_input_tokens': 100, 'output_tokens': 5}


class NativeUsageChecks(unittest.TestCase):
    def metrics(self, path, agent, blocks):
        events = []
        if agent == 'pi':
            messages = []
            if blocks:
                events.append({'type': 'agent_start'})
            for index, block in enumerate(blocks):
                final = index == len(blocks)-1
                message = {'role': 'assistant', 'model': 'fixture',
                           'stopReason': 'stop' if final else 'toolUse', 'content': []}
                if not final:
                    message['content'] = [{'type': 'toolCall', 'id': str(index), 'name': 'read',
                                           'arguments': {'path': 'fixture.txt'}}]
                if block is not MISSING:
                    message['usage'] = block
                messages.append(message)
                events.append({'type': 'message_end', 'message': message})
                if not final:
                    result = {'role': 'toolResult', 'toolCallId': str(index), 'content': []}
                    messages.append(result)
                    events.append({'type': 'message_end', 'message': result})
            if blocks:
                events += [{'type': 'agent_end', 'willRetry': False, 'messages': messages},
                           {'type': 'agent_settled', 'aborted': False}]
        else:
            for block in blocks:
                event = {'type': 'turn.completed'}
                if block is not MISSING:
                    event['usage'] = block
                events.append(event)
        raw = ''.join(json.dumps(event)+'\n' for event in events)
        (path/'stdout.jsonl').write_text(raw)
        metrics = adapters.metrics(agent, path)
        self.assertEqual((path/'stdout.jsonl').read_text(), raw)
        return metrics

    def assert_missing_row(self, path, agent, known):
        metrics = self.metrics(path, agent, [known, MISSING])
        # Both red controls fail here on baseline, before any new-field access.
        self.assertIsNone(metrics['provider_usage']['input_tokens'])
        self.assertIsNone(metrics['provider_usage']['output_tokens'])
        self.assertEqual(metrics['partial_reported_usage_sums']['input_tokens'], 110)
        self.assertEqual(metrics['partial_reported_usage_sums']['output_tokens'], 5)
        self.assertEqual(metrics['usage_fields_reported_by_rows']['input_tokens'], 1)
        self.assertEqual(metrics['expected_usage_rows'], 2)
        self.assertEqual(metrics['native_usage'], [known])
        self.assertEqual(metrics['model_requests'], 2 if agent == 'pi' else None)

    def test_pi_missing_whole_usage_row_is_not_a_complete_total(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assert_missing_row(Path(directory), 'pi', PI)

    def test_codex_missing_whole_usage_row_is_not_a_complete_total(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assert_missing_row(Path(directory), 'codex', CODEX)

    def test_complete_values_and_native_rows_are_preserved(self):
        for agent, known in [('pi', PI), ('codex', CODEX)]:
            with self.subTest(agent=agent), tempfile.TemporaryDirectory() as directory:
                metrics = self.metrics(Path(directory), agent, [known, known])
                self.assertEqual(metrics['native_usage'], [known, known])
                self.assertEqual(metrics['expected_usage_rows'], 2)
                for field, value in [('input_tokens', 220), ('cached_input_tokens', 200), ('output_tokens', 10)]:
                    self.assertEqual(metrics['provider_usage'][field], value)
                    self.assertEqual(metrics['partial_reported_usage_sums'][field], value)
                    self.assertEqual(metrics['usage_fields_reported_by_rows'][field], 2)
                if agent == 'pi':
                    self.assertIsNone(metrics['provider_usage']['reasoning_output_tokens'])
                    self.assertEqual(metrics['usage_fields_reported_by_rows']['reasoning_output_tokens'], 0)

    def test_absent_invalid_empty_and_zero_observations_remain_distinct(self):
        for agent, known in [('pi', PI), ('codex', CODEX)]:
            variants = [MISSING, None, False, 0, [], 'invalid', {}]
            for missing in variants:
                with self.subTest(agent=agent, missing=repr(missing)), tempfile.TemporaryDirectory() as directory:
                    metrics = self.metrics(Path(directory), agent, [known, missing])
                    self.assertIsNone(metrics['provider_usage']['input_tokens'])
                    self.assertEqual(metrics['partial_reported_usage_sums']['input_tokens'], 110)
                    self.assertEqual(metrics['usage_fields_reported_by_rows']['input_tokens'], 1)
                    self.assertEqual(metrics['native_usage'], [known]+([missing] if isinstance(missing, dict) else []))
            for blocks in [[], [MISSING], [{}]]:
                with self.subTest(agent=agent, no_observations=len(blocks)), tempfile.TemporaryDirectory() as directory:
                    metrics = self.metrics(Path(directory), agent, blocks)
                    self.assertIsNone(metrics['provider_usage']['input_tokens'])
                    self.assertIsNone(metrics['partial_reported_usage_sums']['input_tokens'])
                    self.assertEqual(metrics['usage_fields_reported_by_rows']['input_tokens'], 0)
                    self.assertEqual(metrics['expected_usage_rows'], len(blocks))
            zero = {key: 0 for key in known}
            with tempfile.TemporaryDirectory() as directory:
                metrics = self.metrics(Path(directory), agent, [zero, MISSING])
                self.assertIsNone(metrics['provider_usage']['input_tokens'])
                self.assertEqual(metrics['partial_reported_usage_sums']['input_tokens'], 0)
                self.assertEqual(metrics['usage_fields_reported_by_rows']['input_tokens'], 1)

    def test_pi_composite_input_requires_components_in_the_same_row(self):
        blocks = [{'input': 10}, {'cacheRead': 100, 'cacheWrite': 0}, PI]
        with tempfile.TemporaryDirectory() as directory:
            metrics = self.metrics(Path(directory), 'pi', blocks)
            self.assertIsNone(metrics['provider_usage']['input_tokens'])
            self.assertEqual(metrics['partial_reported_usage_sums']['input_tokens'], 110)
            self.assertEqual(metrics['usage_fields_reported_by_rows']['input_tokens'], 1)
            self.assertEqual(metrics['partial_reported_usage_sums']['uncached_input_tokens'], 20)
            self.assertEqual(metrics['partial_reported_usage_sums']['cached_input_tokens'], 200)
            self.assertEqual(metrics['native_usage'], blocks)
        for invalid in [None, True, 1.5, '10']:
            with self.subTest(invalid=invalid), tempfile.TemporaryDirectory() as directory:
                metrics = self.metrics(Path(directory), 'pi', [PI, {**PI, 'input': invalid}])
                self.assertIsNone(metrics['provider_usage']['input_tokens'])
                self.assertEqual(metrics['partial_reported_usage_sums']['input_tokens'], 110)
                self.assertEqual(metrics['usage_fields_reported_by_rows']['input_tokens'], 1)
                self.assertEqual(metrics['provider_usage']['output_tokens'], 10)

    def test_sum_usage_wrapper_preserves_existing_value_policy(self):
        cases = [[], [{}], [{'v': 0}], [{'v': 1}, {'v': 2}], [{'v': -3}, {'v': 1}],
                 [{'v': True}], [{'v': 1.0}], [{'v': '1'}], [{'v': None}], [{'v': 1}, {}]]
        for rows in cases:
            with self.subTest(rows=rows):
                values = [row.get('v') for row in rows]
                expected = sum(values) if values and all(type(value) is int for value in values) else None
                self.assertEqual(adapters.sum_usage(rows, {'tokens': 'v'}), {'tokens': expected})

    def test_short_and_long_reports_keep_partial_evidence_without_full_totals(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root/'fixture_case_1_pi'
            path.mkdir()
            metrics = self.metrics(path, 'pi', [PI, MISSING])
            result = {'agent': 'pi', 'model': 'fixture', 'reasoning': 'high', 'pipeline_ok': True,
                      'returncode': 0, 'timed_out': False, 'expected_turns': 1, 'wall_seconds': 1.0,
                      'prompt_sha256': 'fixture', 'fixture_files_sha256': {'fixture.txt': 'same'}, 'metrics': metrics}
            (path/'result.json').write_text(json.dumps(result))
            (path/'grading.json').write_text(json.dumps({'passed': True}))
            before = {file.name: file.read_bytes() for file in path.iterdir()}
            short = report.read_rows([root], [])[0]
            long = long_report.read_trial(path, 'case', 'pi', 1)
            for row in [short, long]:
                self.assertTrue(row['pass'])
                self.assertTrue(row['usage_complete'])
                self.assertIsNone(row['tokens']['input_tokens'])
                self.assertEqual(row['observed_partial_usage']['input_tokens'], 110)
            short_summary = report.summarize([short])[0]
            long_summary = long_report.summarize([long], ['pi'], ['case'])[0]
            self.assertIsNone(short_summary['input'])
            self.assertIsNone(long_summary['tokens']['input_tokens'])
            self.assertEqual(short_summary['known_token_runs']['input_tokens'], 0)
            self.assertEqual(short_summary['observed_token_sums']['input_tokens'], 110)
            self.assertEqual(short_summary['observed_token_sums'], long_summary['observed_token_sums'])
            self.assertEqual(before, {file.name: file.read_bytes() for file in path.iterdir()})
            direct = usage_report.project_usage(path, result, metrics)
            self.assertEqual(direct['observed_partial_usage'], short['observed_partial_usage'])


if __name__ == '__main__':
    unittest.main()

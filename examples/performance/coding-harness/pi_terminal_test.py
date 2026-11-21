"""Offline Pi terminal-generation controls; no native CLI or model execution."""
import json
from pathlib import Path
import tempfile
import unittest

import adapters
import long_report
import runner
import usage_report


class PiTerminalChecks(unittest.TestCase):
    def attempt(self, stop='stop', *, retry=False):
        message = {'role': 'assistant', 'model': 'fixture', 'stopReason': stop,
                   'content': [], 'usage': {'input': 10, 'cacheRead': 2, 'cacheWrite': 0, 'output': 3}}
        if stop in ('error', 'aborted'):
            message['errorMessage'] = 'synthetic 503' if stop == 'error' else 'synthetic abort'
        return [{'type': 'agent_start'},
                {'type': 'message_start', 'message': {'role': 'assistant', 'stopReason': 'pending'}},
                {'type': 'message_end', 'message': message},
                {'type': 'agent_end', 'willRetry': retry, 'messages': [message]}]

    def settled(self, aborted=False):
        return {'type': 'agent_settled', 'aborted': aborted}

    def evaluate(self, path, events, expected=1, **overrides):
        (path/'stdout.jsonl').write_text(''.join(json.dumps(event)+'\n' for event in events))
        sessions = path/'runtime/pi-sessions'
        sessions.mkdir(parents=True, exist_ok=True)
        (sessions/'fixture.jsonl').write_text(json.dumps({'type': 'thinking_level_change',
                                                        'thinkingLevel': 'high'})+'\n')
        metrics = adapters.metrics('pi', path)
        result = {'agent': 'pi', 'model': 'fixture', 'reasoning': 'high', 'returncode': 0,
                  'timed_out': False, 'metrics': metrics, 'expected_turns': expected,
                  'continuity_verified': True}
        result.update(overrides)
        runner.check_pipeline(result)
        return result, usage_report.project_usage(path, result, metrics)

    def test_recovered_error_is_not_terminal_failure(self):
        recovered = self.attempt()
        events = (self.attempt('error', retry=True)
                  + [{'type': 'auto_retry_start', 'attempt': 1}]
                  + recovered[:-1]
                  + [{'type': 'auto_retry_end', 'success': True, 'attempt': 1}]
                  + recovered[-1:]+[self.settled()])
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            result, usage = self.evaluate(path, events)
            self.assertTrue(result['pipeline_ok'])
            self.assertEqual(result['metrics']['errors'], ['synthetic 503'])
            self.assertEqual(result['metrics']['model_requests'], 2)
            self.assertEqual(result['metrics']['pi_outcome']['observed_stop_reasons'], ['error', 'stop'])
            self.assertTrue(usage['usage_complete'])
            self.assertEqual(usage['reported_usage']['input_tokens'], 24)
            teardown, fallback = self.evaluate(path, events, returncode=1)
            self.assertFalse(teardown['pipeline_ok'])
            self.assertTrue(fallback['usage_complete'])

    def test_terminal_length_is_not_clean_success(self):
        with tempfile.TemporaryDirectory() as directory:
            result, usage = self.evaluate(Path(directory), self.attempt('length')+[self.settled()])
            self.assertFalse(result['pipeline_ok'])
            self.assertEqual(result['metrics']['pi_outcome']['settlements'],
                             [{'stop_reason': 'length', 'aborted': False}])
            failures = result['metrics']['errors']
            self.assertIn('model_output_limit', long_report.failure_taxonomy(result, {'passed': True}, failures)['classes'])
            self.assertTrue(result['metrics']['settled'])
            self.assertFalse(usage['usage_complete'])
            self.assertIsNone(usage['tokens']['input_tokens'])
            self.assertEqual(usage['reported_usage']['input_tokens'], 12)
            self.assertEqual(usage['observed_partial_usage']['input_tokens'], 12)

    def test_recovered_length_keeps_diagnostics_without_terminal_limit_failure(self):
        events = (self.attempt('length')
                  + [{'type': 'compaction_start', 'reason': 'overflow'},
                     {'type': 'compaction_end', 'reason': 'overflow', 'aborted': False, 'willRetry': True}]
                  + self.attempt()+[self.settled()])
        with tempfile.TemporaryDirectory() as directory:
            result, usage = self.evaluate(Path(directory), events)
            self.assertTrue(result['pipeline_ok'])
            self.assertEqual(result['metrics']['pi_outcome']['observed_stop_reasons'], ['length', 'stop'])
            self.assertEqual(result['metrics']['errors'], [])
            self.assertNotIn('model_output_limit', long_report.failure_taxonomy(result, {'passed': True}, [])['classes'])
            self.assertTrue(usage['usage_complete'])

    def test_tool_use_can_reach_clean_answer_in_one_low_level_run(self):
        tool = self.attempt('toolUse')
        call = {'type': 'toolCall', 'id': 'call-1', 'name': 'read', 'arguments': {'path': 'fixture.txt'}}
        tool[2]['message']['content'] = [call]
        answer = self.attempt()
        result_message = {'role': 'toolResult', 'toolCallId': 'call-1', 'content': []}
        events = (tool[:-1]
                  + [{'type': 'message_end', 'message': result_message},
                     {'type': 'turn_end'}, {'type': 'turn_start'}]
                  + answer[1:-1]
                  + [{'type': 'agent_end', 'willRetry': False,
                      'messages': [tool[2]['message'], result_message, answer[2]['message']]}, self.settled()])
        with tempfile.TemporaryDirectory() as directory:
            result, usage = self.evaluate(Path(directory), events)
            self.assertTrue(result['pipeline_ok'])
            self.assertTrue(usage['usage_complete'])
            self.assertEqual(result['metrics']['model_requests'], 2)
            self.assertEqual(result['metrics']['all_tool_calls'], 1)
            self.assertEqual(result['metrics']['pi_outcome']['observed_stop_reasons'], ['toolUse', 'stop'])

    def test_each_requested_run_requires_its_own_successful_settlement(self):
        clean = self.attempt()+[self.settled()]
        failed = self.attempt('error')+[self.settled()]
        cases = [(clean+clean, 2, True), (failed+clean, 2, False),
                 (clean, 2, False), (clean+clean, 1, False),
                 (clean, 0, False), (clean, -1, False), (clean, True, False), (clean, '1', False)]
        for events, expected, success in cases:
            with self.subTest(expected=expected, events=len(events)), tempfile.TemporaryDirectory() as directory:
                result, usage = self.evaluate(Path(directory), events, expected)
                self.assertEqual(result['pipeline_ok'], success)
                self.assertEqual(usage['usage_complete'], success)

    def test_missing_or_invalidated_terminal_evidence_is_not_success(self):
        completed = self.attempt()
        cases = [completed,
                 completed+[{'type': 'agent_start'}, self.settled()],
                 completed+[{'type': 'message_start', 'message': {'role': 'assistant'}}, self.settled()],
                 completed+[{'type': 'message_update', 'assistantMessageEvent': {'type': 'text_delta'}}, self.settled()],
                 completed+[{'type': 'message_start', 'message': {'role': 'user'}}, self.settled()],
                 completed+[self.settled(), {'type': 'agent_start'}],
                 completed+[self.settled(True)],
                 completed+[{'type': 'agent_settled'}],
                 completed[1:]+[self.settled()],
                 completed[:-1]+[{'type': 'agent_end', 'willRetry': False}, self.settled()],
                 completed[:-1]+[{'type': 'agent_end', 'willRetry': False, 'messages': {}}, self.settled()],
                 [{'type': 'agent_end', 'willRetry': False}, self.settled()]]
        cases += [self.attempt(stop)+[self.settled()] for stop in
                  ['error', 'aborted', 'pending', 'toolUse', 'deferred', 'future-stop', None]]
        cases += [self.attempt(retry=True)+[self.settled()]]
        for index, events in enumerate(cases):
            with self.subTest(case=index), tempfile.TemporaryDirectory() as directory:
                result, usage = self.evaluate(Path(directory), events)
                self.assertFalse(result['pipeline_ok'])
                self.assertFalse(usage['usage_complete'])

    def test_pipeline_guards_remain_separate_from_generation_completion(self):
        events = self.attempt()+[self.settled()]
        overrides = [{'returncode': 1}, {'timed_out': True}, {'model': 'different'},
                     {'reasoning': 'low'}, {'expected_turns': 2, 'continuity_verified': False}]
        for changes in overrides:
            with self.subTest(changes=changes), tempfile.TemporaryDirectory() as directory:
                result, _ = self.evaluate(Path(directory), events, **changes)
                self.assertFalse(result['pipeline_ok'])
        with tempfile.TemporaryDirectory() as directory:
            events = (self.attempt()
                      + [{'type': 'compaction_start', 'reason': 'threshold'},
                         {'type': 'compaction_end', 'reason': 'threshold', 'errorMessage': 'synthetic failure'}]
                      + [self.settled()])
            result, usage = self.evaluate(Path(directory), events, returncode=1)
            self.assertFalse(result['pipeline_ok'])
            self.assertTrue(usage['usage_complete'])

    def test_other_arm_pipeline_policies_are_unchanged(self):
        codex = {'agent': 'codex', 'model': 'fixture', 'reasoning': 'high', 'returncode': 0,
                 'metrics': {'settled': True, 'errors': [], 'models': ['fixture'], 'reasoning_efforts': ['high']}}
        runner.check_pipeline(codex)
        self.assertTrue(codex['pipeline_ok'])
        codex['metrics']['errors'] = ['synthetic failure']
        runner.check_pipeline(codex)
        self.assertFalse(codex['pipeline_ok'])
        proteus = {'agent': 'proteus', 'model': 'fixture', 'reasoning': 'high', 'returncode': 0,
                   'metrics': {'model_refs': [json.dumps({'model': 'fixture'})],
                               'reasoning_refs': [json.dumps({'effort': 'high'})],
                               'settlements': [{'status': 'success', 'error': None}],
                               'model_requests': 2, 'model_responses': 2,
                               'failures': [{'kind': 'model', 'message': 'recovered synthetic error'}]}}
        runner.check_pipeline(proteus)
        self.assertTrue(proteus['pipeline_ok'])

    def test_stored_pipeline_success_fast_path_is_not_regraded(self):
        with tempfile.TemporaryDirectory() as directory:
            result = {'agent': 'pi', 'pipeline_ok': True, 'timed_out': False}
            usage = usage_report.project_usage(Path(directory), result, {'provider_usage': {'input_tokens': 12}})
            self.assertTrue(usage['usage_complete'])
            self.assertEqual(usage['tokens']['input_tokens'], 12)


if __name__ == '__main__':
    unittest.main()

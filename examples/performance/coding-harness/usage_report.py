"""Shared usage evidence projection for short and long harness reports."""
import json

from pi_terminal import pi_outcome, successful_pi_generations

TOKENS = ('input_tokens', 'cached_input_tokens', 'output_tokens', 'reasoning_output_tokens')


def read_json(path):
    return json.loads(path.read_text()) if path.is_file() else None


def json_lines(path):
    if path.is_file():
        with path.open(errors='replace') as stream:
            for line in stream:
                try:
                    value = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if isinstance(value, dict):
                    yield value


def known_sum(values):
    return sum(values) if values and all(type(value) is int for value in values) else None


def usage_completion(path, result, metrics):
    if result.get('pipeline_ok') and not result.get('timed_out'):
        return True, 'The complete task pipeline settled and exported its reported usage.'
    agent = result.get('agent')
    if agent == 'proteus':
        failures = metrics.get('failures', [])
        all_answered = (type(metrics.get('model_requests')) is int
                        and metrics.get('model_requests') == metrics.get('model_responses'))
        settled = metrics.get('settlements') or []
        if all_answered and settled and all(item.get('status') == 'success' for item in settled) and not any(
            isinstance(item, dict) and item.get('kind') == 'model' for item in failures
        ):
            return True, 'Every canonical model exchange and task settlement completed before the CLI teardown error.'
    elif agent == 'pi':
        outcome = pi_outcome(json_lines(path/'stdout.jsonl'))
        if successful_pi_generations(outcome, result.get('expected_turns', 1)):
            return True, 'Each requested Pi run settled after a clean final generation; CLI postprocessing can still fail.'
    elif agent == 'opencode':
        native = read_json(path/'runtime/native/usage.json') or {}
        primary = native.get('assistant_messages', [])
        if (metrics.get('settled') and primary and primary[-1].get('finish') == 'stop'
                and sum(msg.get('finish') == 'stop' for msg in primary) >= result.get('expected_turns', 1)
                and not metrics.get('errors')):
            return True, 'The native session succeeded and its final assistant stopped before the CLI teardown error.'
    return False, 'The generation or task did not settle; recorded usage is a lower bound and can omit the active request.'


def project_usage(path, result, metrics):
    """Retain reported lower bounds separately from lifecycle-complete totals."""
    complete, reason = usage_completion(path, result, metrics)
    reported = dict(metrics.get('provider_usage', {}))
    partial = metrics.get('partial_reported_usage_sums', {})
    observed = {key: value if type(value) is int else partial.get(key)
                for key, value in {**{key: None for key in TOKENS}, **reported}.items()}
    return {'usage_complete': complete, 'usage_completeness_reason': reason,
            'reported_usage': reported, 'observed_partial_usage': observed,
            'tokens': dict(reported) if complete else {key: None for key in reported}}


def summarize_usage(rows):
    """Aggregate the selected runs without promoting partial evidence to totals."""
    values = {key: [row['tokens'].get(key) for row in rows] for key in TOKENS}
    return {'tokens': {key: known_sum(items) for key, items in values.items()},
            'known_token_runs': {key: sum(value is not None for value in items)
                                 for key, items in values.items()},
            'observed_token_sums': {key: sum(
                row['tokens'][key] if type(row['tokens'].get(key)) is int else
                row.get('observed_partial_usage', {}).get(key) or 0 for row in rows)
                for key in TOKENS}}

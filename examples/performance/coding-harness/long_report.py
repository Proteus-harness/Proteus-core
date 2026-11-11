"""Aggregate the extended DeepSeek trials, retaining failures and missing evidence."""
import argparse
from collections import Counter
import hashlib
import importlib.util
import json
from pathlib import Path
import re
from statistics import median

HERE = Path(__file__).resolve().parent
CORE = HERE.parents[2]
DATA = CORE / 'target/performance/coding-harness'
CASES = ('db-wal-recovery', 'write-compressor', 'schemelike-metacircular-eval',
         'journal-segments', 'sampler-phase-report', 'ledger-recovery', 'go-review')
ARMS = ('pi', 'proteus-pi', 'opencode', 'proteus-opencode', 'proteus-codex')
DESCRIPTIONS = {
    'db-wal-recovery': 'Восстановление SQLite WAL и сверка БД с recovered.json',
    'write-compressor': 'Анализ C-декодера и сжатие исходного текста до 2500 байт',
    'schemelike-metacircular-eval': 'Интерпретатор Scheme, исполняющий тестовые программы и себя',
    'journal-segments': 'Архивирование настоящего canonical journal и совместимость consumers',
    'sampler-phase-report': 'Агрегация process sampler с PID reuse, фазами и взвешенным CPU',
    'ledger-recovery': 'Многофайловое хранилище с crash recovery, миграцией и concurrent writers',
    'go-review': 'Три последовательных запроса на SGF parser, territory counting и интеграцию',
}
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


def midpoint(values):
    return round(median(values), 3) if values and all(value is not None for value in values) else None


def relative(path):
    try:
        return str(path.resolve().relative_to(CORE))
    except ValueError:
        return str(path)


def canonical_records(session):
    spec = importlib.util.spec_from_file_location('long_journal_reader', HERE.parent/'history-scale/journal.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.records(session)


def audit_trial(path, agent, expected_turns=1, continuity_verified=False):
    """Inspect only synthetic model/tool traces, without exporting tool arguments."""
    calls = []
    inputs = []
    compactions = None
    messages = []
    output_limits = set()
    model_failure_times = []
    turn_basis = 'canonical journal turn_id' if agent == 'proteus' else 'native Pi user anchors' if agent == 'pi' else None
    turn_ids = set()
    turns = {}
    tool_errors = set()
    if agent == 'proteus':
        journals = list((path/'runtime/sessions').rglob('journal.jsonl'))
        if len(journals) != 1:
            return {'trace_available': False, 'reason': 'one canonical journal required'}
        compactions = 0
        for record, payload in canonical_records(journals[0].parent):
            kind = record['kind']
            if kind == 'turn_opened':
                turn_ids.add(record['turn_id'])
                turns[record['turn_id']] = {'requests': 0, 'actions': 0, 'usage': []}
            elif kind == 'model_request_recorded':
                messages.append(len(payload['request']['messages']))
                cap = payload['request'].get('limits', {}).get('max_output_tokens')
                if cap is not None:
                    output_limits.add(cap)
                if record.get('turn_id') in turns:
                    turns[record['turn_id']]['requests'] += 1
            elif kind == 'model_response_recorded':
                outcome = payload.get('outcome', {})
                if outcome.get('status') != 'response':
                    model_failure_times.append(record['timestamp_ms'])
                usage = outcome.get('response', {}).get('usage') or {}
                if type(usage.get('input_tokens')) is int:
                    inputs.append(usage['input_tokens'])
                if record.get('turn_id') in turns:
                    turns[record['turn_id']]['usage'].append(usage)
            elif kind == 'tool_result_recorded' and not payload['result']['ok']:
                tool_errors.add(payload['result']['call_id'])
            elif kind == 'history_mutated' and payload.get('compaction'):
                compactions += 1
            elif kind == 'tool_call_recorded' and payload['phase']['phase'] == 'requested':
                calls.append(payload['call'])
                if record.get('turn_id') in turns:
                    turns[record['turn_id']]['actions'] += 1
    elif agent == 'pi':
        sessions = list((path/'runtime/pi-sessions').glob('*.jsonl'))
        compactions = 0
        for session in sessions:
            current_turn = None
            for event in json_lines(session):
                if event.get('type') == 'compaction':
                    compactions += 1
                elif event.get('type') == 'message' and event.get('message', {}).get('role') == 'user':
                    turn_ids.add(event.get('id'))
                    current_turn = event.get('id')
                    turns[current_turn] = {'requests': 0, 'actions': 0, 'usage': []}
                elif event.get('type') == 'message' and event.get('message', {}).get('role') == 'assistant' and current_turn in turns:
                    msg = event['message']
                    turns[current_turn]['requests'] += 1
                    turns[current_turn]['actions'] += sum(part.get('type') == 'toolCall' for part in msg.get('content', []))
                    usage = msg.get('usage', {})
                    fields = [usage.get(k) for k in ('input', 'cacheRead', 'cacheWrite')]
                    turns[current_turn]['usage'].append({'input_tokens': known_sum(fields),
                                                        'cached_input_tokens': usage.get('cacheRead'),
                                                        'output_tokens': usage.get('output')})
        for event in json_lines(path/'stdout.jsonl'):
            if event.get('type') == 'tool_execution_end' and event.get('isError'):
                tool_errors.add(event.get('toolCallId'))
            msg = event.get('message', {})
            if event.get('type') == 'message_end' and msg.get('role') == 'assistant':
                usage = msg.get('usage', {})
                fields = [usage.get(k) for k in ('input', 'cacheRead', 'cacheWrite')]
                if all(type(value) is int for value in fields):
                    inputs.append(sum(fields))
                calls.extend({'id': part.get('id'), 'name': part.get('name'), 'args': part.get('arguments')}
                             for part in msg.get('content', []) if part.get('type') == 'toolCall')
    elif agent == 'opencode':
        native = read_json(path/'runtime/native/usage.json') or {}
        compactions = native.get('compaction_messages')
        turn_ids = set(range(sum(native.get('user_turns', {}).values())))
        for msg in native.get('assistant_messages', []):
            tokens = msg.get('tokens') or {}
            cache = tokens.get('cache') or {}
            values = [tokens.get('input'), cache.get('read'), cache.get('write')]
            if all(type(value) is int for value in values):
                inputs.append(sum(values))
        if expected_turns > 1 and continuity_verified and len(native.get('sessions', [])) == 1:
            primary = native.get('assistant_messages', [])
            created = [msg.get('time', {}).get('created') for msg in primary]
            one_session = all(msg.get('session_id') == native['sessions'][0]['id'] for msg in primary)
            if one_session and all(type(value) is int for value in created) and len(set(created)) == len(created):
                groups, current = [], []
                for msg in sorted(primary, key=lambda item: item['time']['created']):
                    current.append(msg)
                    if msg.get('finish') == 'stop':
                        groups.append(current)
                        current = []
                if not current and len(groups) == expected_turns and all(
                    all(msg.get('finish') == 'tool-calls' for msg in group[:-1]) for group in groups
                ):
                    turn_basis = 'native OpenCode assistant groups ending in stop; primary usage only'
                    for index, group in enumerate(groups):
                        usage = []
                        for msg in group:
                            tokens = msg.get('tokens') or {}
                            cache = tokens.get('cache') or {}
                            usage.append({'input_tokens': known_sum([tokens.get('input'), cache.get('read'), cache.get('write')]),
                                          'cached_input_tokens': cache.get('read'),
                                          'output_tokens': known_sum([tokens.get('output'), tokens.get('reasoning')])})
                        turns[index] = {'requests': len(group), 'actions': sum(len(msg.get('tools', [])) for msg in group), 'usage': usage}
        for event in json_lines(path/'stdout.jsonl'):
            if event.get('type') == 'tool_use':
                part = event.get('part', {})
                if part.get('state', {}).get('status') == 'error':
                    tool_errors.add(part.get('id'))
                calls.append({'id': part.get('id'), 'name': part.get('tool'),
                              'args': part.get('state', {}).get('input')})
    flags = []
    for call in calls:
        args = call.get('args') or {}
        command = args.get('command', args.get('cmd', '')) if isinstance(args, dict) else ''
        paths = [args.get(k, '') for k in ('path', 'filePath', 'file_path')] if isinstance(args, dict) else []
        reasons = []
        if isinstance(command, str) and re.search(r'\b(curl|wget)\b|\bgit\s+clone\b|\bpip[3]?\s+install\b|urlopen\(|requests\.(get|post)\(', command):
            reasons.append('possible tool network access')
        if any('/grader' in value for value in [command, *paths] if isinstance(value, str)):
            reasons.append('reference to external grader path')
        if reasons:
            encoded = json.dumps(args, ensure_ascii=False, sort_keys=True).encode()
            flags.append({'tool': call.get('name'), 'call_id': call.get('id'), 'reasons': reasons,
                          'arguments_sha256': hashlib.sha256(encoded).hexdigest()})
    turn_metrics = [{'number': index+1, 'requests': data['requests'], 'actions': data['actions'],
                     'tokens': {field: known_sum([usage.get(field) for usage in data['usage']])
                                for field in ('input_tokens', 'cached_input_tokens', 'output_tokens')}}
                    for index, data in enumerate(turns.values())]
    return {'trace_available': True, 'turn_metrics': turn_metrics, 'turn_metrics_basis': turn_basis, 'requested_output_limits': sorted(output_limits), 'model_failure_timestamps_ms': model_failure_times, 'trace_tool_errors': len(tool_errors), 'tool_calls_scanned': len(calls), 'compactions': compactions,
            'observed_user_turns': len(turn_ids) if turn_ids else None,
            'peak_provider_input_tokens': max(inputs) if inputs else None,
            'peak_canonical_request_messages': max(messages) if messages else None,
            'review_flags': flags}


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
        completed = 0
        failed = False
        for event in json_lines(path/'stdout.jsonl'):
            if event.get('type') == 'agent_end' and not event.get('willRetry'):
                completed += 1
            message = event.get('message', {})
            if event.get('type') == 'message_end' and message.get('role') == 'assistant':
                failed |= message.get('stopReason') in ('error', 'aborted', 'pending')
        if completed >= result.get('expected_turns', 1) and not failed:
            return True, 'Each requested Pi turn emitted a final agent_end before the CLI error.'
    elif agent == 'opencode':
        native = read_json(path/'runtime/native/usage.json') or {}
        primary = native.get('assistant_messages', [])
        if (metrics.get('settled') and primary and primary[-1].get('finish') == 'stop'
                and sum(msg.get('finish') == 'stop' for msg in primary) >= result.get('expected_turns', 1)
                and not metrics.get('errors')):
            return True, 'The native session succeeded and its final assistant stopped before the CLI teardown error.'
    return False, 'The generation or task did not settle; recorded usage is a lower bound and can omit the active request.'


def failure_taxonomy(result, grade, failures):
    classes = []
    text = json.dumps(failures, ensure_ascii=False)
    if result.get('timed_out'):
        classes.append('agent_deadline')
    elif 'length limit' in text:
        classes.append('model_output_limit')
    elif not result.get('pipeline_ok'):
        transport = re.search('UnsupportedContentType|UND_ERR_SOCKET|ECONNRESET|SocketError|terminated', text, flags=re.I)
        classes.append('transport_or_cli_failure' if transport else 'pipeline_failure')
    tests = []
    if grade:
        if grade.get('timed_out'):
            classes.append('grader_deadline')
        elif not grade.get('passed'):
            if grade.get('fixture_unchanged') is False:
                classes.append('immutable_fixture_modified')
            if grade.get('returncode'):
                classes.append('solution_rejected_by_tests')
        tests = re.findall(r'^(?:FAILED|ERROR) [^\n]*?::([^ \n]+)', grade.get('output', ''), flags=re.M)
    if grade and not grade.get('passed') and re.search('SyntaxError|ModuleNotFoundError|ImportError', grade.get('output', '')):
        classes.append('solution_import_or_syntax')
    rules = [
        ('symlink|path|directory|escape', 'filesystem_path_validation'),
        ('atomic|replace|fsync|snapshot|interruption|torn|crash', 'atomicity_or_recovery'),
        ('checksum|sha256|hash|metadata|manifest|first_seq|last_seq|sequence|ambiguous|missing_formats', 'journal_format_validation'),
        ('concurrent|writers|lock|dedup|duplicate', 'concurrency_or_deduplication'),
        ('reuse|identity|pid', 'process_identity'),
        ('phase|gap|cpu|counter|memory|tail', 'sampler_aggregation'),
        ('public|consumer|legacy|existing', 'compatibility_regression'),
        ('invalid|nonjson|boolean|coordinate|error', 'input_validation'),
        ('sibling|isolated|surrounded|review', 'go_position_integration'),
    ]
    semantic = [label for pattern, label in rules if any(re.search(pattern, name, flags=re.I) for name in tests)]
    notes = []
    if len(tests) == 1 and 'cannot_publish_partial_archive[truncated]' in tests[0]:
        notes.append('Запись legacy без обязательного завершающего LF принята, хотя контракт требует ValueError.')
    if any('nonjson_cycle' in name for name in tests) and grade and 'RecursionError' in grade.get('output', ''):
        notes.append('Циклическое значение Python вызвало RecursionError вместо публичного LedgerError.')
    if any('sibling_positions_are_isolated' in name or 'surrounded_white_region' in name for name in tests):
        notes.append('Интеграция вернула неправильные territory counts при успешных отдельных parser и counting tests.')
    return {'classes': classes, 'failed_tests': tests, 'semantic_requirements': semantic, 'notes': notes,
            'error_codes': sorted(set(re.findall('UND_ERR_SOCKET|UnsupportedContentType|ECONNRESET', text)))}


def read_trial(path, case, arm, attempt):
    result = read_json(path/'result.json')
    if result is None:
        return {'case': case, 'arm': arm, 'attempt': attempt, 'state': 'running' if path.exists() else 'pending',
                'source': relative(path)}
    grade = read_json(path/'grading.json')
    metrics = result.get('metrics', {})
    failures = metrics.get('failures', metrics.get('errors', [])) or []
    kinds = Counter(item.get('kind', 'unknown') if isinstance(item, dict) else 'native_error' for item in failures)
    row = {'case': case, 'arm': arm, 'attempt': attempt,
           'state': 'graded' if grade is not None else 'awaiting_grader', 'source': relative(path),
           'model': result.get('model'), 'effort': result.get('reasoning'),
           'pipeline_ok': bool(result.get('pipeline_ok')), 'grade': grade.get('passed') if grade else None,
           'pass': bool(result.get('pipeline_ok') and grade and grade.get('passed')),
           'returncode': result.get('returncode'), 'timed_out': result.get('timed_out', False),
           'seconds': round(result['wall_seconds'], 3) if 'wall_seconds' in result else None,
           'actions': metrics.get('all_tool_calls'), 'requests': metrics.get('model_requests'),
           'primary_requests': metrics.get('primary_model_requests',
                              sum(item.get('origin') == 'direct' for item in metrics['requests'])
                              if metrics.get('requests') else metrics.get('model_requests')),
           'usage_includes_auxiliary': metrics.get('usage_includes_auxiliary_requests', False),
           'tokens': metrics.get('provider_usage', {}),
           'observed_partial_usage': metrics.get('partial_reported_usage_sums', {}),
           'failures_by_kind': dict(kinds), 'failure_count': len(failures), 'tools': metrics.get('tool_names', {}),
           'compactions': metrics.get('compactions'), 'model_verified': result.get('model_verified', False),
           'reasoning_verified': result.get('reasoning_verified', False),
           'auxiliary_model_verified': metrics.get('registry', {}).get('auxiliary_model_verified') if arm == 'opencode' else None,
           'auth_removed': result.get('auth_removed'), 'expected_turns': result.get('expected_turns', 1),
           'continuity_verified': result.get('continuity_verified'), 'turns': result.get('turns', []),
           'prompt_sha256': result.get('prompt_sha256'),
           'repo_sha256': hashlib.sha256(json.dumps(result['fixture_files_sha256'], sort_keys=True).encode()).hexdigest(),
           'binary_sha256': result.get('binary_sha256'),
           'model_catalog_limits': metrics.get('model_selection', {}),
           'native_model_limits': metrics.get('registry', {}).get('model', {}).get('limit')}
    if result.get('agent') == 'opencode':
        row['native_transport_failure_timestamps_ms'] = [event['timestamp'] for event in metrics.get('errors', [])
                                                        if event.get('error', {}).get('type') == 'provider.transport']
        native = read_json(path/'runtime/native/usage.json') or {}
        error_times = [event['timestamp'] for event in metrics.get('errors', []) if
                       event.get('error', {}).get('message') == 'UnsupportedContentType']
        primary = native.get('assistant_messages', [])
        created = [msg.get('time', {}).get('created') for msg in primary]
        if error_times and created and all(type(stamp) is int for stamp in created):
            stamp = error_times[-1]
            completed = [msg.get('time', {}).get('completed') for msg in primary]
            before_error = [value for value in completed if type(value) is int and value <= stamp]
            row['native_cli_error_timing'] = {
                'code': 'UnsupportedContentType', 'timestamp_ms': stamp,
                'seconds_after_first_assistant_created': round((stamp-min(created))/1000, 3),
                'last_assistant_age_seconds': round((stamp-max(created))/1000, 3),
                'last_completed_before_error_age_seconds': round((stamp-max(before_error))/1000, 3) if before_error else None,
                'outer_timed_out': result.get('timed_out', False)}
    row['usage_complete'], row['usage_completeness_reason'] = usage_completion(path, result, metrics)
    row['reported_usage'] = dict(row['tokens'])
    row['observed_partial_usage'] = {key: value if type(value) is int else
                                   row['observed_partial_usage'].get(key)
                                   for key, value in {**{key: None for key in TOKENS}, **row['reported_usage']}.items()}
    if not row['usage_complete']:
        row['tokens'] = {key: None for key in row['tokens']}
    row['failure_taxonomy'] = failure_taxonomy(result, grade, failures)
    row['length_limit_failures'] = sum(isinstance(item, dict) and 'length limit' in json.dumps(item) for item in failures)
    try:
        row['audit'] = audit_trial(path, result['agent'], row['expected_turns'], row['continuity_verified'])
    except (OSError, ValueError, KeyError) as error:
        row['audit'] = {'trace_available': False, 'reason': str(error)}
    if row['compactions'] is None:
        row['compactions'] = row['audit'].get('compactions')
    if grade:
        row['grading_timed_out'] = grade.get('timed_out')
        row['fixture_unchanged'] = grade.get('fixture_unchanged')
        counts = re.findall(r'(\d+) (passed|failed|error)', grade.get('output', ''))
        row['test_counts'] = {kind: int(count) for count, kind in counts}
    return row


def summarize(rows, arms, expected_cases):
    stats = []
    for arm in arms:
        selected = [row for row in rows if row['arm'] == arm and row['attempt'] == 1]
        done = [row for row in selected if row['state'] == 'graded']
        observed = [row for row in selected if 'tokens' in row]
        values = {key: [row['tokens'].get(key) for row in observed] for key in TOKENS}
        stats.append({'arm': arm, 'expected_tasks': len(expected_cases), 'graded': len(done),
                      'passed': sum(row['pass'] for row in done),
                      'grade_passed': sum(row['grade'] is True for row in done),
                      'pipeline_failed_with_task_pass': sum(row['grade'] is True and not row['pipeline_ok'] for row in done), 'failed': sum(not row['pass'] for row in done),
                      'timed_out': sum(row.get('timed_out', False) for row in done),
                      'observed_runs': len(observed),
                      'tokens': {key: known_sum(items) for key, items in values.items()},
                      'known_token_runs': {key: sum(value is not None for value in items) for key, items in values.items()},
                      'observed_token_sums': {key: sum(
                          row['tokens'][key] if type(row['tokens'].get(key)) is int else
                          row.get('observed_partial_usage', {}).get(key) or 0 for row in observed) for key in TOKENS},
                      'median_seconds': midpoint([row.get('seconds') for row in done]),
                      'median_actions': midpoint([row.get('actions') for row in done]),
                      'median_requests': midpoint([row.get('requests') for row in done]),
                      'median_primary_requests': midpoint([row.get('primary_requests') for row in done]),
                      'actions': known_sum([row.get('actions') for row in observed]),
                      'requests': known_sum([row.get('requests') for row in observed]),
                      'primary_requests': known_sum([row.get('primary_requests') for row in observed]),
                      'compactions': known_sum([row.get('compactions') for row in observed]),
                      'failure_count': sum(row.get('failure_count', 0) for row in observed)})
    return stats


def paired_rows(rows, cases):
    result = []
    for native, proteus in [('pi', 'proteus-pi'), ('opencode', 'proteus-opencode')]:
        for case in cases:
            left = next((row for row in rows if row['case'] == case and row['attempt'] == 1 and row['arm'] == native), {})
            right = next((row for row in rows if row['case'] == case and row['attempt'] == 1 and row['arm'] == proteus), {})
            known = left.get('state') == right.get('state') == 'graded'
            item = {'case': case, 'native': native, 'proteus': proteus, 'both_graded': known,
                    'native_pass': left.get('pass'), 'proteus_pass': right.get('pass')}
            for field in ('actions', 'requests', 'primary_requests', 'seconds'):
                a, b = left.get(field), right.get(field)
                item[field + '_delta'] = round(b-a, 3) if known and a is not None and b is not None else None
            a = left.get('tokens', {}).get('input_tokens')
            b = right.get('tokens', {}).get('input_tokens')
            item['input_delta'] = b-a if known and a is not None and b is not None else None
            item['input_delta_percent'] = round(100*(b-a)/a, 3) if known and a and b is not None else None
            result.append(item)
    return result


def build(run_root, case_roots, cases, arms, baseline, repeat_cases, control_root=None, retry_root=None):
    planned = [(case, 1) for case in cases]
    planned.extend((case, 2) for case in repeat_cases)
    rows = [read_trial(run_root/f'flash_{case}_{attempt}_{arm}', case, arm, attempt)
            for case, attempt in planned for arm in arms]
    identities = {}
    for row in rows:
        if 'prompt_sha256' in row:
            identities.setdefault(row['case'], set()).add((row['prompt_sha256'], row['repo_sha256']))
    if any(len(variants) > 1 for variants in identities.values()):
        raise ValueError('Paired trial prompt or repository drift')
    controls = []
    for case in cases:
        found = [root/case for root in case_roots if (root/case/'case.json').is_file()]
        if not found:
            controls.append({'case': case, 'state': 'not_prepared'})
            continue
        path = found[-1]
        info = read_json(path/'case.json')
        starter, gold = read_json(path/'starter.json'), read_json(path/'gold.json')
        controls.append({'case': case, 'source': relative(path), 'family': info.get('family'),
                         'source_rev': info.get('source_rev'), 'adaptations': info.get('adaptations'),
                         'source_url': info.get('source_url'), 'grader_timeout_seconds': info.get('grader_timeout_seconds', 120),
                         'repo_files': len(info['repo_hashes']),
                         'starter_failed': starter is not None and starter['passed'] is False,
                         'gold_passed': gold is not None and gold['passed'] is True,
                         'grader_sha256': hashlib.sha256(json.dumps(info['grader_hashes'], sort_keys=True).encode()).hexdigest()})
    old = read_json(baseline) or {'rows': []}
    baseline_rows = [row for row in old['rows'] if row.get('attempt') == 1]
    baseline_stats = []
    for arm in arms:
        selected = [row for row in baseline_rows if row['arm'] == arm]
        baseline_stats.append({'arm': arm, 'tasks': len(selected),
                               'median_seconds': midpoint([row.get('seconds') for row in selected]),
                               'median_actions': midpoint([row.get('actions') for row in selected]),
                               'median_requests': midpoint([row.get('requests') for row in selected])})
    expected_paths = {str(run_root/f'flash_{case}_{attempt}_{arm}') for case, attempt in planned for arm in arms}
    unplanned = [relative(path.parent) for path in run_root.glob('*/result.json')
                 if str(path.parent) not in expected_paths]
    replay_checks = []
    for path in sorted((DATA/'long-replay-01').glob('*/audit.json')):
        audit = read_json(path)
        replay_checks.append({'source': relative(path), **{key: audit.get(key) for key in (
            'audit_ok', 'matched', 'model_exchanges', 'tool_calls', 'workspace_unchanged', 'session_files_unchanged',
            'journal_sha256_before', 'journal_sha256_after', 'network', 'source_mounts')}})
    cap_controls = []
    if control_root and control_root.is_dir():
        for path in sorted(control_root.glob('flash_*')):
            if path.is_dir():
                _, case, attempt, arm = path.name.split('_')
                cap_controls.append(read_trial(path, case, arm, int(attempt)))
    retry_selection = read_json(retry_root/'selection.json') if retry_root else None
    socket_retries = []
    if retry_selection:
        for _, case, attempt, arm in retry_selection['rows']:
            socket_retries.append(read_trial(retry_root/f'flash_{case}_{attempt}_{arm}', case, arm, attempt))
    eligible = {(case, arm, attempt) for _, case, attempt, arm in retry_selection['rows']} if retry_selection else set()
    clustered = [row for row in rows if (row['case'], row['arm'], row['attempt']) in eligible
                 and 'UND_ERR_SOCKET' in row.get('failure_taxonomy', {}).get('error_codes', [])]
    cluster_times = [stamp for row in clustered for stamp in row.get('audit', {}).get('model_failure_timestamps_ms', [])]
    return {'format': 1, 'model': 'opencode-go/deepseek-v4.1-flash', 'effort': 'high',
            'run_root': relative(run_root), 'planned_agent_deadline_seconds': 1200, 'socket_retries': socket_retries, 'socket_retry_selection': retry_selection,
            'socket_failure_observation': {'primary_rows': len(clustered), 'timestamps_ms': cluster_times,
                'native_opencode_transport_timestamps_ms': [stamp for row in rows for stamp in row.get('native_transport_failure_timestamps_ms', [])],
                'span_seconds': round((max(cluster_times)-min(cluster_times))/1000, 3) if cluster_times else None,
                'source': 'Failure timestamps in canonical journals; correlation alone does not identify the network component.'}, 'output_cap_controls': cap_controls, 'replay_checks': replay_checks,
            'output_cap_control_proof': read_json(control_root/'summary.json') if control_root else None, 'complete': all(row['state'] == 'graded' for row in [*rows, *cap_controls, *socket_retries]),
            'primary_complete': all(row['state'] == 'graded' for row in rows),
            'expected_diagnostic_trials': len(cap_controls) + len(socket_retries),
            'expected_trials': len(rows), 'unplanned_results': unplanned, 'controls': controls, 'summary': summarize(rows, arms, cases),
            'baseline_six_tasks': baseline_stats, 'pairs': paired_rows(rows, cases), 'rows': rows,
            'audit_flags': [{'case': row['case'], 'arm': row['arm'], 'attempt': row['attempt'], **flag}
                            for row in rows for flag in row.get('audit', {}).get('review_flags', [])]}


def write_markdown(report, output):
    n = lambda value: '—' if value is None else f'{value:,}'.replace(',', ' ')
    rows = report['rows']
    done = [row for row in rows if row['state'] == 'graded']
    lines = ['# Более длинные задачи на DeepSeek Flash', '',
             f"Проверено {len(done)} из {report['expected_trials']} запланированных запусков. "
             f"Pipeline и независимый grader прошли {sum(row['pass'] for row in done)}. "
             f"Для запусков выбраны {report['model']} и effort {report['effort']}.", '',
             f"Дополнительно сохранены {len(report['output_cap_controls'])} проверки output cap и "
             f"{len(report['socket_retries'])} свежих повторов после коррелированных socket failures. "
             'Они приведены отдельными таблицами и сохраняют первоначальные оценки.', '',
             'Таблица суммирует первые попытки. Контрольный повтор учитывается отдельными строками в JSON. '
             'Input включает cache read, output включает reasoning. Полное число запросов OpenCode неизвестно, '
             'его primary requests видны в assistant records, total usage включает служебные обращения.', '',
             '| Агент | Grader PASS | Полный PASS / проверено | Тулы | Запросы | Primary requests | Input | Cached input | Output | Медиана с |',
             '|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|']
    for row in report['summary']:
        displayed = {key: n(value) if value is not None else
                     '≥' + n(row['observed_token_sums'][key]) if row['observed_token_sums'][key] else '—'
                     for key, value in row['tokens'].items()}
        lines.append(f"| {row['arm']} | {row['grade_passed']} | {row['passed']}/{row['graded']} | {n(row['actions'])} | {n(row['requests'])} | "
                     f"{n(row['primary_requests'])} | {displayed['input_tokens']} | {displayed['cached_input_tokens']} | "
                     f"{displayed['output_tokens']} | {n(row['median_seconds'])} |")
    lines += ['', '| Задача | Требование | Starter FAIL | Gold PASS | Файлов в starter |',
              '|---|---|---|---|---:|']
    for control in report['controls']:
        case = control['case']
        lines.append(f"| {case} | {DESCRIPTIONS.get(case, case)} | {control.get('starter_failed', False)} | "
                     f"{control.get('gold_passed', False)} | {n(control.get('repo_files'))} |")
    lines += ['', 'В WAL grader добавлена независимая проверка реального чтения восстановленной БД и совпадения всех '
              'строк с recovered.json. В compressor сохраняются исходные data.txt и decomp.c, generated ELF decomp '
              'разрешено перекомпилировать. Authoritative grader заново собирает декодер из исходного C и сравнивает '
              'байты результата. Первоначальная защита hash ELF давала ложное отклонение при перекомпиляции. '
              'Обе коррекции применены одинаково ко всем пяти результатам задачи без нового обращения к модели. '
              'Прежние grades сохранены рядом с обновлёнными. Journal suite закреплён на исходных 37 тестах серии.']
    lines += ['', 'Знак ≥ обозначает сумму только зарегистрированного usage при отсутствующем полном total. '
              'Пропущенные token fields остаются неизвестными. JSON хранит число полных наблюдений и сумму '
              'доступных значений, чтобы незавершённый или частично записанный запуск сохранялся в учёте.', '',
              '| Задача | Pi | Proteus Pi | OpenCode | Proteus OpenCode | Proteus Codex |',
              '|---|---|---|---|---|---|']
    for case in CASES:
        cells = []
        for arm in ARMS:
            row = next((row for row in rows if row['case'] == case and row['arm'] == arm and row['attempt'] == 1), None)
            if not row or row['state'] != 'graded':
                cells.append('ожидается')
            else:
                status = 'PASS' if row['pass'] else 'TIMEOUT' if row.get('timed_out') else 'FAIL'
                if row['grade'] is True and not row['pipeline_ok']:
                    status = 'TASK PASS / ' + ('TIMEOUT' if row.get('timed_out') else 'CLI FAIL')
                cells.append(f"{status}, {n(row['actions'])} тулов, {n(row['seconds'])} с")
        lines.append('| ' + ' | '.join([case, *cells]) + ' |')
    repeated = [row for row in rows if row['attempt'] > 1]
    if repeated:
        lines += ['', 'Свежий повтор go-review использует те же три пользовательских запроса в новой сессии.', '',
                  '| Агент | Полный PASS сначала / повтор | Тулы сначала / повтор | Input сначала / повтор | Continuity повтора |',
                  '|---|---|---:|---:|---|']
        for repeat in repeated:
            first = next(row for row in rows if row['case'] == repeat['case'] and row['arm'] == repeat['arm'] and row['attempt'] == 1)
            if repeat['state'] == 'graded':
                lines.append(f"| {repeat['arm']} | {first.get('pass')} / {repeat['pass']} | "
                             f"{n(first.get('actions'))} / {n(repeat['actions'])} | "
                             f"{n(first.get('tokens', {}).get('input_tokens'))} / {n(repeat['tokens'].get('input_tokens'))} | "
                             f"{repeat.get('continuity_verified')} |")
            else:
                lines.append(f"| {repeat['arm']} | {first.get('pass')} / ожидается | — | — | — |")
    failed = [row for row in done if not row['pass']]
    if failed:
        lines += ['', '| Задача и попытка | Агент | Причина | Не пройденные требования |',
                  '|---|---|---|---|']
        for row in failed:
            taxonomy = row['failure_taxonomy']
            tests = taxonomy['failed_tests']
            names = ', '.join(tests[:3]) + (f", ещё {len(tests)-3}" if len(tests) > 3 else '')
            requirement = (' '.join(taxonomy['notes']) or ', '.join(taxonomy['semantic_requirements']) or names
                           or ('Решение прошло grader, процесс завершился ошибкой' if row['grade'] is True else
                               'Grader не завершился в свой deadline' if row.get('grading_timed_out') else
                               'Grader отклонил результат, подробности сохранены в grading.json'))
            lines.append(f"| {row['case']} / {row['attempt']} | {row['arm']} | {', '.join(taxonomy['classes'])} | {requirement} |")
        lines += ['', 'JSON содержит имена всех упавших tests. Частично записанное решение проверяется тем же grader '
                  'после timeout или transport failure. Совпадение этой проверки с PASS не меняет факт ошибки процесса.']
    native_errors = [row for row in rows if row.get('native_cli_error_timing')]
    if native_errors:
        elapsed = [row['native_cli_error_timing']['seconds_after_first_assistant_created'] for row in native_errors]
        lines += ['', f"В {len(native_errors)} первичных native OpenCode trials UnsupportedContentType появился через "
                  f"{n(min(elapsed))}–{n(max(elapsed))} с после первого assistant message. "
                  'Плановый внешний deadline равен 1200 с, outer timed_out во всех этих записях false. '
                  'Точные timestamps и возраст последнего model request сохранены в JSON. Источник этого ограничения '
                  'по данным серии не установлен.']
    if report.get('socket_retries'):
        cluster = report['socket_failure_observation']
        lines += ['', f"В первичной серии {cluster['primary_rows']} ошибок UND_ERR_SOCKET зарегистрированы в интервале "
                  f"{n(cluster['span_seconds'])} с. Близость времени — наблюдение, конкретный источник обрыва не установлен. "
                  'Для заранее выбранных пяти клеток выполнен один свежий повтор с тем же заданием и бюджетом.', '',
                  '| Задача | Агент | Полный PASS повтора | Grader PASS | Тулы | Запросы | Секунды |',
                  '|---|---|---|---|---:|---:|---:|']
        for row in report['socket_retries']:
            if row['state'] == 'graded':
                lines.append(f"| {row['case']} | {row['arm']} | {row['pass']} | {row['grade']} | "
                             f"{n(row['actions'])} | {n(row['requests'])} | {n(row['seconds'])} |")
            else:
                lines.append(f"| {row['case']} | {row['arm']} | ожидается | — | — | — | — |")
        lines += ['', 'Первичные результаты, timeout, code failures и отдельные cap controls сохраняют собственные оценки. '
                  'Повторы не заменяют их в сводной таблице первых попыток.']
    if report.get('replay_checks'):
        lines += ['', 'Отдельные записанные ходы проверены offline replay.', '',
                  '| Запись | Model exchanges | Tool calls | Matched | Workspace и session сохранены |',
                  '|---|---:|---:|---|---|']
        for audit in report['replay_checks']:
            model = audit.get('model_exchanges') or {}
            tools = audit.get('tool_calls') or {}
            lines.append(f"| {audit['source']} | {model.get('recorded')}/{model.get('replayed')} | "
                         f"{tools.get('recorded')}/{tools.get('replayed')} | {audit.get('matched')} | "
                         f"{audit.get('workspace_unchanged') and audit.get('session_files_unchanged')} |")
    if report.get('output_cap_controls'):
        lines += ['', 'Дополнительная проверка output cap проводится в свежих сессиях с тем же заданием и '
                  'cap 384000. Эти результаты учитываются отдельно от первых попыток.', '',
                  '| Задача | Полный PASS | Input | Output | Запросы | Тулы | Секунды |',
                  '|---|---|---:|---:|---:|---:|---:|']
        for row in report['output_cap_controls']:
            if row['state'] == 'graded':
                displayed = {key: n(row['tokens'].get(key)) if row['tokens'].get(key) is not None else
                             '≥' + n(row['observed_partial_usage'][key]) if row['observed_partial_usage'].get(key) else '—'
                             for key in ('input_tokens', 'output_tokens')}
                lines.append(f"| {row['case']} | {row['pass']} | {displayed['input_tokens']} | "
                             f"{displayed['output_tokens']} | {n(row['requests'])} | {n(row['actions'])} | {n(row['seconds'])} |")
            else:
                lines.append(f"| {row['case']} | ожидается | — | — | — | — | — |")
        proof = report.get('output_cap_control_proof') or {}
        verified = [item.get('control_verified', False) for item in proof.get('cases', [])]
        if verified:
            lines += ['', f"Сравнение первых canonical requests подтвердило неизменность материалов, tools, модели и "
                      f"effort при замене cap во всех {sum(verified)}/{len(verified)} контролях. "
                      'Последующая траектория модели может различаться. Ошибки контрольных запусков и полнота usage '
                      'сохранены отдельно в JSON.']
    lines += ['', 'Подготовлены независимые starter/gold controls. Состояние каждого и hashes grader сохранены '
              'в JSON. Отказ grader и ошибка CLI отдельно влияют на итоговый PASS. Native Codex через этот '
              'Go endpoint ранее получил HTTP 403, поэтому здесь проверяется только профиль Proteus Codex '
              'с явно указанным Chat Completions transport и JSON patch surface.', '',
              '| Агент | Медиана тулов, прежние 6 | Медиана тулов, новый набор | Медиана секунд, прежние 6 | Медиана секунд, новый набор |',
              '|---|---:|---:|---:|---:|']
    for old, new in zip(report['baseline_six_tasks'], report['summary']):
        lines.append(f"| {new['arm']} | {n(old['median_actions'])} | {n(new['median_actions'])} | "
                     f"{n(old['median_seconds'])} | {n(new['median_seconds'])} |")
    peaks = [row.get('audit', {}).get('peak_provider_input_tokens') for row in rows]
    peak = max((value for value in peaks if value is not None), default=None)
    limits = sorted({limit for row in rows for limit in row.get('audit', {}).get('requested_output_limits', [])})
    length_failures = sum(row.get('length_limit_failures', 0) for row in rows)
    known_compactions = [row['compactions'] for row in rows if type(row.get('compactions')) is int]
    lines += ['', f"В {len(known_compactions)} из {len(rows)} первичных записей известен compaction count, "
              f"зарегистрировано {sum(known_compactions)} compactions. Остальные counts неизвестны. "
              'Provider context window равен 1 000 000 токенов, давление на полное окно в этой серии не достигнуто.']
    lines += ['', 'Прежний набор и расширенный набор содержат разные задачи. OpenCode adapter после прежнего '
              'среза получил исправления argument preparation и scheduling. Разность медиан описывает эти '
              'наблюдаемые серии, причинный эффект одной сложности отдельно не измерялся.', '',
              f"Canonical requests содержат output limits {limits}. Зарегистрировано {length_failures} model failures из-за length limit. "
              'Descriptor или лимит native модели и реально запрошенный cap приведены отдельно в JSON каждого trial. '
              'Такие отказы учитываются в первом результате, дальнейшая проверка с другими лимитами образует отдельный эксперимент.', '',
              f"Наибольший зарегистрированный input одного запроса — {n(peak)} токенов. "
              'Это provider usage для полного входа запроса. Compaction берётся из canonical history, '
              'Pi session entries или отдельно сохранённого native OpenCode count. Отсутствующее поле OpenCode '
              'помечено неизвестным.', '',
              f"Автоматический аудит отметил {len(report['audit_flags'])} потенциальных обращений к сети или "
              'внешнему grader. Поля аргументов не публикуются, проверяемые вызовы обозначены hashes. '
              'Срабатывание требует просмотра соответствующего synthetic trace.', '',
              '[Данные всех запусков](2026-11-11-deepseek-long.json). '
              'Прогоны использовали fresh home/workspace и локальные native runtime versions из предыдущего сравнения. '
              'Плановый общий deadline задачи равен 1200 с. Точные команды находятся в result.json каждого запуска. '
              'Время включает запуск и завершение агента, независимый grader выполняется после него. '
              'Измерения сделаны при параллельных запусках и зависят от нагрузки provider.', '']
    output.write_text('\n'.join(lines))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--runs', type=Path, default=DATA/'long-flash-01')
    p.add_argument('--case-root', type=Path, action='append')
    p.add_argument('--cases', nargs='+', default=list(CASES))
    p.add_argument('--arms', nargs='+', default=list(ARMS))
    p.add_argument('--repeat-case', action='append', default=None)
    p.add_argument('--retry-root', type=Path, default=DATA/'long-socket-retry-01')
    p.add_argument('--control-root', type=Path, default=DATA/'long-cap-control/validated')
    p.add_argument('--baseline', type=Path, default=DATA/'flash-verified-metrics.json')
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--markdown', type=Path)
    p.add_argument('--require-complete', action='store_true')
    args = p.parse_args()
    case_roots = args.case_root or [DATA/'long-cases', DATA/'long-cases-final']
    repeat_cases = args.repeat_case if args.repeat_case is not None else (['go-review'] if 'go-review' in args.cases else [])
    if any(case not in args.cases for case in repeat_cases):
        p.error('--repeat-case must name a selected case')
    report = build(args.runs, case_roots, args.cases, args.arms, args.baseline, repeat_cases, args.control_root, args.retry_root)
    if args.require_complete and not report['complete']:
        p.error('primary or diagnostic trials still await a result or grade')
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False)+'\n')
    if args.markdown:
        write_markdown(report, args.markdown)
    print(json.dumps({'complete': report['complete'], 'expected': report['expected_trials'],
                      'graded': sum(row['state'] == 'graded' for row in report['rows']),
                      'passed': sum(row.get('pass', False) for row in report['rows']),
                      'audit_flags': len(report['audit_flags'])}))


if __name__ == '__main__':
    main()

#!/usr/bin/env python3
"""Rerun failed DeepSeek cells and audit one copied-session continuation."""
import argparse
from collections import Counter
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import shutil
from types import SimpleNamespace
import subprocess
import sys
import tempfile

import adapters
import corpus
import matrix
import runner

HERE = Path(__file__).resolve().parent
CORE = HERE.parents[2]
DATA = CORE / 'target/performance/coding-harness'
BASELINE = CORE / 'examples/performance/results/2026-11-11-deepseek-long.json'
MODEL = 'opencode-go/deepseek-v4.1-flash'
COLD_SOURCE = 'flash_sampler-phase-report_1_proteus-pi'
COLD_PROMPT = ('Continue the original task in this same session using the recorded tool results and existing changes. '
               'Inspect and verify what remains, preserve completed work, and finish the original request. '
               'Avoid repeating already completed mutations. You may run the checks needed to verify the result.')
ENV_NAMES = ('LANG', 'LC_ALL', 'HTTPS_PROXY', 'HTTP_PROXY', 'ALL_PROXY', 'NO_PROXY',
             'https_proxy', 'http_proxy', 'all_proxy', 'no_proxy')


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n')


def relative(path):
    return str(Path(path).resolve().relative_to(CORE))


def selection():
    rows = json.loads(BASELINE.read_text())['rows']
    selected = [{'case': row['case'], 'arm': row['arm'], 'attempt': row['attempt'],
                 'source': row['source'], 'baseline_pipeline_ok': row['pipeline_ok'],
                 'baseline_grade': row['grade'], 'baseline_classes': row.get('failure_taxonomy', {}).get('classes', [])}
                for row in rows if row['arm'].startswith('proteus-') and not row['pass']]
    if len(selected) != 10:
        raise ValueError('The frozen comparison requires exactly ten failed Proteus cells')
    for row in selected:
        source = CORE / row['source']
        recorded = json.loads((source / 'result.json').read_text())
        if recorded['model'] != MODEL or recorded['reasoning'] != 'high':
            raise ValueError('Unexpected baseline model or effort')
    return selected


def runtime_snapshot():
    prefix = CORE.parent / 'Proteus-desktop/build/Proteus'
    roots = [prefix/'bin', prefix/'share/pi-runtime', prefix/'share/opencode-runtime']
    hashes = {}
    for root in roots:
        for path in sorted(root.rglob('*')):
            if path.is_file() and not path.is_symlink() and ('node_modules' not in path.parts or path.name == 'package.json'):
                if root.name == 'bin' or path.suffix == '.mjs' or path.name == 'package-lock.json':
                    hashes[str(path.relative_to(prefix))] = runner.digest(path)
    return {'prefix': str(prefix), 'hashes': hashes}


def journal_reader():
    module = adapters.load_module('reconnect_journal', HERE.parent/'history-scale/journal.py')
    return module.records


def request_digest(request):
    # A failed sampling retry should retain the complete accepted input. Only
    # client routing identities can differ between independent host exchanges.
    value = json.loads(json.dumps(request))
    for key in ('session_id', 'thread_id', 'turn_id'):
        value.get('client_metadata', {}).pop(key, None)
    value.get('cache', {}).pop('routing_key', None)
    return hashlib.sha256(json.dumps(value, sort_keys=True, ensure_ascii=False).encode()).hexdigest()


def tool_results(request):
    return [part['payload']['ToolResult']['result']['call_id'] for message in request['messages']
            for part in message['parts'] if 'ToolResult' in part['payload']]


def repeated_arguments(old_rows, new_rows):
    def requested(rows):
        return [payload['call'] for record, payload in rows if record['kind'] == 'tool_call_recorded'
                and payload['phase']['phase'] == 'requested']
    def identity(call):
        return call['name'], hashlib.sha256(json.dumps(call['args'], sort_keys=True).encode()).hexdigest()
    previous = {identity(call) for call in requested(old_rows)}
    return [{'name': call['name'], 'call_id': call['id'], 'arguments_sha256': identity(call)[1]}
            for call in requested(new_rows) if identity(call) in previous]


def trace_evidence(output):
    journals = list((output/'runtime/sessions').rglob('journal.jsonl'))
    if len(journals) != 1:
        return {'error': 'exactly one session journal required'}
    rows = list(journal_reader()(journals[0].parent))
    requests = [(i, row, payload) for i, (row, payload) in enumerate(rows) if row['kind'] == 'model_request_recorded']
    responses = {payload['exchange_id']: payload for row, payload in rows if row['kind'] == 'model_response_recorded'}
    recovery = []
    for number, (index, row, payload) in enumerate(requests):
        outcome = responses.get(payload['exchange_id'], {}).get('outcome', {})
        if outcome.get('status') == 'response':
            continue
        failure = outcome.get('failure') or {}
        kind = failure.get('kind')
        typed = kind == 'stream_disconnected' or isinstance(kind, dict) and 'retryable' in kind
        if not typed:
            continue
        item = {'failed_exchange': payload['exchange_id'], 'failure_kind': kind,
                'prior_confirmed_tool_results': tool_results(payload['request']),
                'reconnected': False}
        if number + 1 < len(requests):
            next_index, _, next_payload = requests[number + 1]
            item.update(next_exchange=next_payload['exchange_id'],
                        same_sampling_input=request_digest(payload['request']) == request_digest(next_payload['request']),
                        next_confirmed_tool_results=tool_results(next_payload['request']),
                        calls_dispatched_between_attempts=sum(record['kind'] == 'tool_call_recorded'
                            and data['phase']['phase'] == 'requested' for record, data in rows[index+1:next_index]),
                        reconnected=True)
        recovery.append(item)
    requested = [payload['call']['id'] for row, payload in rows if row['kind'] == 'tool_call_recorded'
                 and payload['phase']['phase'] == 'requested']
    repeated = [key for key, count in Counter(requested).items() if count > 1]
    return {'retryable_failures': recovery, 'duplicate_requested_call_ids': repeated,
            'model_exchanges': len(requests), 'completed_model_exchanges': len(responses),
            'requested_output_limits': sorted({payload['request']['limits']['max_output_tokens']
                                               for _, _, payload in requests
                                               if payload['request']['limits'].get('max_output_tokens') is not None}),
            'journal_sha256': runner.digest(journals[0]), 'session': relative(journals[0].parent)}


def classify(result, grade):
    metrics = result['metrics']
    failures = metrics.get('failures', [])
    text = json.dumps(failures)
    kinds = []
    if result.get('timed_out'):
        kinds.append('deadline')
    if 'length limit' in text:
        kinds.append('output_limit')
    if 'UND_ERR_SOCKET' in text or 'stream_disconnected' in text:
        kinds.append('transport')
    if not grade['passed']:
        kinds.append('grader_rejected')
    return kinds


def usage_summary(result, metrics):
    observed = {key: value for key, value in (metrics.get('provider_usage') or {}).items()}
    partial = metrics.get('partial_reported_usage_sums') or {}
    minimum = {key: value if type(value) is int else partial.get(key) for key, value in observed.items()}
    model_failed = any(item.get('kind') == 'model' for item in metrics.get('failures', []))
    complete = bool(not result.get('timed_out') and not model_failed
                    and metrics.get('model_requests') == metrics.get('model_responses')
                    and all(type(observed.get(key)) is int for key in ('input_tokens', 'output_tokens')))
    return {'usage_complete': complete,
            'tokens': observed if complete else {key: None for key in observed},
            'observed_partial_usage': minimum}


def fresh_job(row, root, timeout):
    matrix.run((root/'fresh', 'flash', row['case'], row['attempt'], row['arm'], DATA/'long-cases', timeout))
    output = root/'fresh'/f"flash_{row['case']}_{row['attempt']}_{row['arm']}"
    actual = json.loads((output/'result.json').read_text())
    previous = json.loads((CORE/row['source']/'result.json').read_text())
    comparison = {'same_prompt': actual['prompt_sha256'] == previous['prompt_sha256'],
                  'same_fixture': actual['fixture_files_sha256'] == previous['fixture_files_sha256'],
                  'same_model_effort': (actual['model'], actual['reasoning']) == (previous['model'], previous['reasoning']),
                  'evidence': trace_evidence(output)}
    if not all(comparison[key] for key in ('same_prompt', 'same_fixture', 'same_model_effort')):
        raise ValueError('Fresh run differs from its frozen baseline')
    write_json(output/'comparison.json', comparison)
    return str(output)


def prefix_hash(path, length):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        remaining = length
        while remaining:
            data = stream.read(min(1024*1024, remaining))
            if not data:
                raise ValueError('journal prefix was truncated')
            digest.update(data)
            remaining -= len(data)
    return digest.hexdigest()


def incremental_metrics(session, prefix_length, output):
    sliced = output/'new-turn-journal'
    sliced.mkdir()
    lines = (session/'journal.jsonl').read_bytes()[prefix_length:]
    (sliced/'journal.jsonl').write_bytes(lines)
    for row in [json.loads(line) for line in lines.splitlines()]:
        stored = row['payload']
        if stored['storage'] == 'blob':
            target = sliced/stored['relative_path']
            target.parent.mkdir(exist_ok=True)
            shutil.copy2(session/stored['relative_path'], target)
    module = adapters.load_module('reconnect_measurements', HERE.parent/'code-mode/measurements.py')
    metrics = module.measure(sliced)
    providers = set()
    for record, payload in journal_reader()(sliced):
        if record['kind'] == 'model_response_recorded':
            for message in payload.get('outcome', {}).get('response', {}).get('messages', []):
                native = (message.get('metadata') or {}).get('pi_native', {})
                provider = native.get('header', native).get('provider')
                if provider:
                    providers.add(provider)
    metrics['providers'] = sorted(providers)
    metrics['session'] = relative(session)
    return metrics


def cold_job(root, timeout):
    source = DATA/'long-flash-01'/COLD_SOURCE
    destination = root/'cold'/COLD_SOURCE
    destination.mkdir(parents=True, mode=0o700)
    source_before = runner.tree_manifest(source)
    runtime = destination/'runtime'
    runtime.mkdir(mode=0o700)
    workspace = destination/'workspace'
    shutil.copytree(source/'workspace', workspace, symlinks=True)
    (destination/'prompt.txt').write_text(COLD_PROMPT+'\n')
    binary = CORE.parent/'Proteus-desktop/build/Proteus/bin/proteus'
    args = SimpleNamespace(agent='proteus', model=MODEL, reasoning='high', timeout=timeout,
                           binary=binary.resolve(), base_config=CORE/'configs/pi.config.toml',
                           case_dir=DATA/'long-cases/sampler-phase-report', auth_file=None)
    report = {'format': 1, 'agent': 'proteus', 'model': MODEL, 'reasoning': 'high',
              'binary': str(binary), 'binary_sha256': runner.digest(binary),
              'prompt_sha256': hashlib.sha256(COLD_PROMPT.encode()).hexdigest(),
              'fixture_files_sha256': runner.tree_manifest(workspace), 'expected_turns': 1,
              'source': relative(source), 'cold_continuation': True}
    try:
        inner, mounts = adapters.prepare(args, runtime)
        shutil.copytree(source/'runtime/sessions', runtime/'sessions', dirs_exist_ok=True)
        session = next((runtime/'sessions').rglob('journal.jsonl')).parent
        source_session = next((source/'runtime/sessions').rglob('journal.jsonl')).parent
        journal = session/'journal.jsonl'
        prefix_length = journal.stat().st_size
        prefix_digest = runner.digest(journal)
        old_rows = list(journal_reader()(session))
        old_calls = {payload['call']['id'] for row, payload in old_rows
                     if row['kind'] == 'tool_call_recorded' and payload['phase']['phase'] == 'requested'}
        old_turns = [row['turn_id'] for row, _ in old_rows if row['kind'] == 'turn_opened']
        resume = '/runtime/sessions/'+str(session.relative_to(runtime/'sessions'))
        inner[-1:] = ['--resume-session', resume, COLD_PROMPT]
        command = runner.sandbox_command(inner, mounts, workspace, runtime)
        report['command'] = command
        environment = {name:value for name,value in os.environ.items() if name in ENV_NAMES}
        environment['PATH'] = '/usr/bin:/bin'
        report.update(runner.execute(command, destination, timeout, environment))
        report['metrics'] = incremental_metrics(session, prefix_length, destination)
        runner.check_pipeline(report)
        new_rows = list(journal_reader()(session))[len(old_rows):]
        new_calls = [payload['call']['id'] for row,payload in new_rows if row['kind'] == 'tool_call_recorded'
                     and payload['phase']['phase'] == 'requested']
        new_turns = [row['turn_id'] for row,_ in new_rows if row['kind'] == 'turn_opened']
        report['continuation_evidence'] = {
            'source_journal_sha256': runner.digest(source_session/'journal.jsonl'),
            'copied_prefix_bytes': prefix_length, 'copied_prefix_sha256': prefix_digest,
            'prefix_unchanged': prefix_hash(journal,prefix_length) == prefix_digest,
            'source_unchanged': source_before == runner.tree_manifest(source),
            'prior_turn_ids': old_turns, 'new_turn_ids': new_turns,
            'prior_requested_call_ids': sorted(old_calls), 'new_requested_call_ids': new_calls,
            'previous_call_ids_dispatched_again': sorted(old_calls.intersection(new_calls)),
            'previous_argument_signatures_seen_again': repeated_arguments(old_rows, new_rows),
            'trace': trace_evidence(destination),
        }
        proof = report['continuation_evidence']
        proof['verified'] = bool(proof['prefix_unchanged'] and proof['source_unchanged']
                                 and len(new_turns)==1 and not set(old_turns).intersection(new_turns)
                                 and not proof['previous_call_ids_dispatched_again'])
        report['final_files_sha256'] = runner.tree_manifest(workspace)
    finally:
        runner.remove_auth(runtime)
        report['auth_removed'] = True
        write_json(destination/'result.json', report)
    grade = corpus.grade(args.case_dir, workspace, destination/'grading.json')
    print('cold', report.get('pipeline_ok'), 'grade', grade['passed'], flush=True)
    return str(destination)


def regrade_samplers(root):
    """Offline paired audit; preserve original cases, workspaces and grades."""
    baseline = json.loads(BASELINE.read_text())
    sources = {row['source'] for row in baseline['rows'] + baseline.get('socket_retries', [])
               if row['case']=='sampler-phase-report'}
    sources.update(relative(path.parent) for path in (root/'fresh').glob('*sampler-phase-report*/grading.json'))
    if (root/'cold'/COLD_SOURCE/'grading.json').exists():
        sources.add(relative(root/'cold'/COLD_SOURCE))
    case = DATA/'long-cases/sampler-phase-report'
    before_case = runner.tree_manifest(case)
    original_info = json.loads((case/'case.json').read_text())
    fixed_loader = HERE/'long_repo_fixtures/sampler-phase-report/grader/test_outputs.py'
    output = root/'grader-loader-audit'
    output.mkdir(exist_ok=True)
    rows = []
    with tempfile.TemporaryDirectory(prefix='proteus-corrected-loader-') as temporary:
        corrected = Path(temporary)
        shutil.copytree(case/'grader',corrected/'grader')
        shutil.copy2(fixed_loader,corrected/'grader/test_outputs.py')
        info = {**original_info,'grader_hashes':corpus.manifest(corrected/'grader')}
        write_json(corrected/'case.json',info)
        jobs = [(source,CORE/source/'workspace',CORE/source/'grading.json',False)
                for source in sorted(sources)]
        jobs.extend((relative(case)+'/'+name,case/'repo',case/(name+'.json'),reference)
                    for name,reference in [('starter',False),('gold',True)])
        for source, workspace, original_grade, reference in jobs:
            label = source.replace('/','__')
            raw_path = output/(label+'.raw.json')
            corrected_path = output/(label+'.corrected.json')
            shutil.copy2(original_grade,raw_path)
            before = runner.tree_manifest(workspace)
            grade = corpus.grade(corrected,workspace,corrected_path,reference)
            after = runner.tree_manifest(workspace)
            if before != after:
                raise ValueError('Offline grader modified a frozen solution')
            raw = json.loads(original_grade.read_text())
            rows.append({'source':source,'raw_grade':raw['passed'],'corrected_grade':grade['passed'],
                         'changed':raw['passed']!=grade['passed'], 'reference_control':reference,
                         'workspace_unchanged':True,
                         'solution_manifest_sha256':hashlib.sha256(json.dumps(before,sort_keys=True).encode()).hexdigest(),
                         'raw':relative(raw_path),'corrected':relative(corrected_path)})
    if before_case != runner.tree_manifest(case):
        raise ValueError('Offline audit modified the frozen case')
    result = {'model_calls':0,'frozen_case_unchanged':True,
              'reason':'Register module in sys.modules before exec_module so standard dataclass string annotations work.',
              'original_grader_sha256':original_info['grader_hashes']['test_outputs.py'],
              'corrected_grader_sha256':runner.digest(fixed_loader),'rows':rows}
    write_json(output/'audit.json',result)
    return result


def read_results(root):
    selected = json.loads((root/'selection.json').read_text())['rows']
    fresh = []
    for row in selected:
        path = root/'fresh'/f"flash_{row['case']}_{row['attempt']}_{row['arm']}"
        if not (path/'result.json').exists() or not (path/'grading.json').exists():
            fresh.append({**row, 'state':'pending', 'source':relative(path)})
            continue
        result = json.loads((path/'result.json').read_text())
        grade = json.loads((path/'grading.json').read_text())
        baseline_root = CORE/row['source']
        baseline_result = json.loads((baseline_root/'result.json').read_text())
        expected_prompt = (baseline_result.get('turns') or [{}])[0].get('prompt_sha256',baseline_result['prompt_sha256'])
        metrics = result['metrics']
        fresh.append({**row, 'state':'graded', 'source':relative(path), 'pipeline_ok':result['pipeline_ok'],
                      'baseline_artifacts_verified':{
                          'workspace_matches_recorded_final':runner.tree_manifest(baseline_root/'workspace') == baseline_result['final_files_sha256'],
                          'original_prompt_unchanged':runner.digest(baseline_root/'prompt.txt') == expected_prompt},
                      'grade':grade['passed'], 'pass':result['pipeline_ok'] and grade['passed'],
                      'seconds':round(result['wall_seconds'],3), 'timed_out':result.get('timed_out',False),
                      'auth_removed':result.get('auth_removed'),
                      'model_verified':result.get('model_verified'), 'reasoning_verified':result.get('reasoning_verified'),
                      'actions':metrics.get('all_tool_calls'), 'requests':metrics.get('model_requests'),
                      **usage_summary(result, metrics),
                      'comparison':json.loads((path/'comparison.json').read_text()) if (path/'comparison.json').exists() else None,
                      'trace':trace_evidence(path),
                      'model_failures':[item for item in metrics.get('failures',[]) if item.get('kind')=='model'],
                      'failure_classes':classify(result,grade)})
    cold_root = root/'cold'/COLD_SOURCE
    cold = None
    if (cold_root/'result.json').exists() and (cold_root/'grading.json').exists():
        result = json.loads((cold_root/'result.json').read_text())
        cold = {'source':relative(cold_root), 'pipeline_ok':result.get('pipeline_ok'),
                'grade':json.loads((cold_root/'grading.json').read_text())['passed'],
                'seconds':result.get('wall_seconds'), 'metrics':result.get('metrics'),
                'auth_removed':result.get('auth_removed'),
                **usage_summary(result, result['metrics']),
                'grading_correction':json.loads((cold_root/'grading-correction.json').read_text())
                                     if (cold_root/'grading-correction.json').exists() else None,
                'continuation_evidence':result.get('continuation_evidence')}
    audit_path = root/'grader-loader-audit/audit.json'
    audit = json.loads(audit_path.read_text()) if audit_path.exists() else None
    pairs = {row['source']:row for row in audit['rows']} if audit else {}
    for row in fresh:
        pair = pairs.get(row['source'])
        row['corrected_grade'] = pair['corrected_grade'] if pair else row.get('grade')
        baseline_pair = pairs.get(next(item['source'] for item in selected
                                 if (item['case'],item['arm'],item['attempt']) == (row['case'],row['arm'],row['attempt'])))
        row['corrected_baseline_grade'] = baseline_pair['corrected_grade'] if baseline_pair else row['baseline_grade']
    return {'model':MODEL,'effort':'high','source':relative(root), 'rows':fresh, 'cold':cold,
            'sampler_loader_audit':audit,
            'credential_audit':json.loads((root/'credential-audit.json').read_text())
                               if (root/'credential-audit.json').exists() else None,
            'runtime_verification':json.loads((root/'runtime-unchanged.json').read_text())
                                   if (root/'runtime-unchanged.json').exists() else None,
            'cold_replay': json.loads((root/'cold-replay-new-turn/audit.json').read_text())
                           if (root/'cold-replay-new-turn/audit.json').exists() else None,
            'complete':all(row['state']=='graded' for row in fresh) and cold is not None}


def report(root, destination):
    data = read_results(root)
    write_json(destination.with_suffix('.json'),data)
    yes = lambda value: 'да' if value else 'нет'
    seconds = lambda value: f"{round(value):,}".replace(',', ' ')+' с'
    lines = ['# Продолжение Proteus после обрыва', '',
             'Повторены 10 прежних неудачных запусков на DeepSeek V4.1 Flash с effort high. '
             'Каждый запуск получает чистый рабочий каталог и лимит 20 минут. '
             'Для каждой задачи сохранены исходные инструкции, профиль и номер повтора. '
             'В основной таблице указаны результаты исправленного проверяющего.', '',
             '| Задача | Профиль | Повтор | Процесс завершён | Тесты пройдены | Время | Действия | Запросы |',
             '|---|---|---:|---|---|---:|---:|---:|']
    for row in data['rows']:
        if row['state']!='graded':
            lines.append(f"| {row['case']} | {row['arm']} | {row['attempt']} | ожидается | ожидается | — | — | — |")
        else:
            lines.append(f"| {row['case']} | {row['arm']} | {row['attempt']} | {yes(row['pipeline_ok'])} | {yes(row['corrected_grade'])} | {seconds(row['seconds'])} | {row['actions']} | {row['requests']} |")
    done = [row for row in data['rows'] if row['state']=='graded']
    recoveries = [item for row in done for item in row['trace'].get('retryable_failures',[])]
    lines += ['', f"Из {len(done)} завершённых запусков {sum(row['pipeline_ok'] and row['corrected_grade'] for row in done)} "
              'прошли процесс и тесты. '
              f"Успешных проверок кода без завершённого процесса: {sum(row['corrected_grade'] and not row['pipeline_ok'] for row in done)}. "
              'Успешная проверка кода после остановки по времени не считается завершённым процессом.']
    socket = [row for row in done if 'transport_or_cli_failure' in row['baseline_classes']]
    lines += ['', f"Отдельно прежние 5 запусков с обрывом сокета: завершились и прошли тесты "
              f"{sum(row['pipeline_ok'] and row['corrected_grade'] for row in socket)}/{len(socket)}, "
              f"только тесты — {sum(row['corrected_grade'] and not row['pipeline_ok'] for row in socket)}/{len(socket)}. "
              f"Новых реальных сетевых обрывов в этих повторах: {sum(len(row['trace'].get('retryable_failures',[])) for row in socket)}."]
    if not recoveries:
        lines += ['', 'В чистых повторах не наблюдалось сетевых обрывов: изменения оценок сами по себе '
                  'не измеряют эффект автоматического retry. Контролируемая проверка обрыва описана отдельно '
                  'в [recovery-proof](../coding-harness/recovery-proof.md).']
    lines += ['', 'Причины незавершённых запусков:', '']
    for row in done:
        reasons = [text for key,text in [('deadline','лимит 20 минут'),('output_limit','лимит длины ответа')]
                   if key in row['failure_classes']]
        if not row['corrected_grade']:
            reasons.append('тесты не пройдены')
        if reasons:
            lines += [f"- {row['case']} / {row['arm']}: {', '.join(reasons)}."]
    caps = [row for row in done if 'output_limit' in row['failure_classes']]
    if caps:
        lines += ['', f"Лимит длины сработал в {len(caps)} запусках профиля Codex при неизменённом "
                  'ограничении 16 384 токена на ответ. Восстановление сети не исправляет этот отказ.']
    lines += ['', 'Полный расход токенов известен только для завершённых ответов. Знак ≥ обозначает нижнюю границу; '
              'неизвестные токены оборванных ответов не заменены нулями.', '',
              '| Задача / профиль | Полный расход известен | Входные токены | Выходные токены |',
              '|---|---|---:|---:|']
    for row in done:
        tokens = row['tokens'] if row['usage_complete'] else row['observed_partial_usage']
        marker = '' if row['usage_complete'] else '≥'
        lines.append(f"| {row['case']} / {row['arm']} | {yes(row['usage_complete'])} | "
                     f"{marker}{tokens.get('input_tokens')} | {marker}{tokens.get('output_tokens')} |")
    verification=data.get('runtime_verification')
    lines += ['', f"Установленная программа и runtime совпали с начальным снимком: "
              f"{yes(verification.get('unchanged')) if verification else 'ожидается'}. "
              'Инструкции, исходные файлы и model/effort сверены для каждого запуска.']
    if data['cold']:
        cold=data['cold']; proof=cold.get('continuation_evidence') or {}
        lines += ['', f"Продолжение сохранённой сессии использует копию ранее упавшего sampler/ProteusPi. Процесс завершён: {yes(cold['pipeline_ok'])}, тесты пройдены: {yes(cold['grade'])}. "
                  f"Старая история сохранилась: {yes(proof.get('prefix_unchanged'))}, исходная сессия неизменна: {yes(proof.get('source_unchanged'))}, "
                  f"повторно исполненных прежних call IDs {len(proof.get('previous_call_ids_dispatched_again',[]))}."]
        lines += [f"Это отдельный запуск: {seconds(cold['seconds'])}, {cold['metrics']['model_requests']} запросов модели, "
                  f"{cold['metrics']['all_tool_calls']} действий. Его расход не включён в итоги чистых повторов."]
        if cold.get('grading_correction'):
            lines += ['Первый callable grade ошибочно сравнил абсолютный fixture path с относительным workspace. '
                      'Оба пути нормализованы в corpus.grade; тот же frozen output перепроверен без model calls. '
                      'Первый grading JSON сохранён отдельно.']
    if data.get('cold_replay'):
        replay = data['cold_replay']
        exchanges, calls = replay['model_exchanges'], replay['tool_calls']
        lines += ['', f"Воспроизведение нового turn совпало с журналом: {yes(replay.get('matched'))}, "
                  f"запросы модели {exchanges['replayed']}/{exchanges['recorded']}, действия {calls['replayed']}/{calls['recorded']}. "
                  'Replay выполнялся с отключённой сетью и read-only исходными файлами.']
    if data.get('sampler_loader_audit'):
        lines += ['', 'Парный offline regrade исправляет только регистрацию импортируемого модуля в sys.modules. '
                  'Это позволяет стандартному dataclass читать строковые annotations. '
                  'Grader выполнялся с временной копии; исходные решения, prompt, grader и защищённые файлы сохранены. '
                  'Новых запросов модели: 0. Исходные оценки сохранены в следующей таблице и JSON.', '',
                  '| Задача | Профиль | Старый grader до / после | Исправленный grader до / после |',
                  '|---|---|---|---|']
        for row in data['rows']:
            if row['state']=='graded':
                lines.append(f"| {row['case']} | {row['arm']} | {yes(row['baseline_grade'])} / {yes(row['grade'])} | "
                             f"{yes(row['corrected_baseline_grade'])} / {yes(row['corrected_grade'])} |")
        lines += ['', f"По исправленному grader: {sum(row.get('corrected_grade',False) for row in done)}/{len(done)} "
                  f"решений прошли тесты, {sum(row['pipeline_ok'] and row.get('corrected_grade',False) for row in done)}/{len(done)} "
                  'прошли одновременно pipeline и тесты. Изменения оценок controls/native/cold перечислены в JSON audit.']
    lines += ['', 'История инструментов в retry подтверждается записанными canonical requests и call IDs. '
              'Перезапрос генерации до принятого terminal response повторяет текущую попытку модели. '
              'Нативный OpenCode также умеет сохранять частичный assistant и продолжать новым сообщением. '
              'Этот дополнительный native lifecycle остаётся отдельной границей совместимости.', '',
              'Pi 1.5.2 реализует автоматическое продолжение в AgentSession с тремя паузами 2, 4 и 8 секунд. '
              '[AgentSession](https://github.com/earendil-works/pi/blob/v1.5.2/packages/coding-agent/src/core/agent-session.ts), '
              '[настройки](https://github.com/earendil-works/pi/blob/v1.5.2/packages/coding-agent/src/core/settings-defaults.ts).', '',
              'OpenCode различает Retry и Continue в runner, общая policy допускает до десяти повторов. '
              '[Runner](https://github.com/anomalyco/opencode/blob/527f0b931d1f9b3ebd34e106c51b31ce5db5b075/packages/core/src/session/runner/llm.ts), '
              '[policy](https://github.com/anomalyco/opencode/blob/527f0b931d1f9b3ebd34e106c51b31ce5db5b075/packages/core/src/session/runner/retry.ts).', '',
              'Codex 0.167.0 по умолчанию допускает пять восстановлений стрима и пересобирает запрос из принятой истории. '
              '[Sampling](https://github.com/openai/codex/blob/086ccc833bc9cf01024ad2dd6af7dbd421b6bf9a/codex-rs/core/src/session/turn.rs), '
              '[recovery](https://github.com/openai/codex/blob/086ccc833bc9cf01024ad2dd6af7dbd421b6bf9a/codex-rs/core/src/responses_retry.rs).', '',
              'Claude Code при --resume восстанавливает завершённые tool calls и пропускает вызов, '
              'который выполнялся во время обрыва. '
              '[Ошибки](https://code.claude.com/docs/en/errors#the-response-above-may-be-incomplete), '
              '[восстановление сессии](https://code.claude.com/docs/en/sessions#what-a-resumed-session-restores).', '',
              'В официальных release notes Droid v0.231.0 описано отбрасывание незавершённых calls, '
              'а v0.242.0 добавляет retries для усечённых streams. '
              '[Release notes](https://docs.factory.com/changelog/release-notes). '
              'Эти описания не подтверждают универсальный exactly-once и не являются сравнением производительности.', '',
              'Полнота usage и фактически наблюдавшиеся reconnect exchanges находятся в JSON. '
              'Новый семантически одинаковый вызов с новым ID может быть намеренным действием модели. '
              'Для незаписанного внешнего эффекта после аварии процесса сохранение истории не доказывает единственность исполнения.']
    destination.with_suffix('.md').write_text('\n'.join(lines)+'\n')
    return data


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,default=DATA/'reconnect-2026-11-12-01')
    parser.add_argument('--run',action='store_true',help='authorize this invocation to spend DeepSeek quota')
    parser.add_argument('--report',type=Path,help='write JSON and Markdown from existing results')
    parser.add_argument('--regrade-sampler-loader',action='store_true',help='offline paired regrade of frozen sampler outputs')
    parser.add_argument('--jobs',type=int,default=5,choices=range(1,6))
    parser.add_argument('--timeout',type=int,default=1200,choices=[1200])
    args=parser.parse_args()
    args.output = args.output.resolve()
    if args.regrade_sampler_loader:
        result = regrade_samplers(args.output)
        print('offline pairs',len(result['rows']),'changed',sum(row['changed'] for row in result['rows']))
        return 0
    if args.report:
        data=report(args.output.resolve(),args.report.resolve())
        print('complete',data['complete'])
        return 0 if data['complete'] else 1
    args.output.mkdir(parents=True,exist_ok=False)
    rows=selection()
    write_json(args.output/'selection.json',{'rows':rows,'model':MODEL,'effort':'high','timeout':args.timeout,
                                             'max_attempts_per_cell':1,'cold_source':COLD_SOURCE})
    write_json(args.output/'runtime-snapshot.json',runtime_snapshot())
    if not args.run:
        print('Prepared selection without model calls',args.output)
        return 0
    (args.output/'fresh').mkdir()
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        futures = [pool.submit(cold_job,args.output,args.timeout)]
        futures.extend(pool.submit(fresh_job,row,args.output,args.timeout) for row in rows)
        for future in concurrent.futures.as_completed(futures):
            future.result()
    after=runtime_snapshot()
    before=json.loads((args.output/'runtime-snapshot.json').read_text())
    write_json(args.output/'runtime-unchanged.json',{'unchanged':before==after,'after':after})
    return 0


if __name__=='__main__':
    raise SystemExit(main())

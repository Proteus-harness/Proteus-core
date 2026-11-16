#!/usr/bin/env python3
"""Paired native-agent Polyglot runs with external feedback and durable receipts."""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import sqlite3
import subprocess
import threading
import time
import tomllib
from types import SimpleNamespace

import adapters
import open_code_native
import polyglot_corpus as corpus
import polyglot_grader as grader
import runner

HERE = Path(__file__).resolve().parent
CORE, DATA = corpus.CORE, corpus.DATA
AGENTS = ('proteus', 'opencode')
NETWORK_RULE = ('The official test code and ready-made solutions must not be fetched from '
                'external sources. Internet access for tools is unavailable. Use the supplied '
                'files, declared dependencies and your own smoke tests. Official test feedback '
                'is provided after you finish the turn. Prepared compilers and dependencies '
                'are available locally; do not download or install alternatives. For Java '
                'use the installed gradle --offline rather than downloading a wrapper '
                'distribution. For Rust use cargo --offline.\n\n')
BINARIES = {'proteus': CORE.parent/'Proteus-desktop/build/Proteus/bin/proteus',
            'opencode': Path.home()/'.opencode/bin/opencode'}
MODEL, EFFORT = 'opencode-go/deepseek-v4.1-flash', 'high'
GRADE_LOCK = threading.Semaphore(2)
PROXY_NAMES = ('LANG', 'LC_ALL', 'HTTPS_PROXY', 'HTTP_PROXY', 'ALL_PROXY', 'NO_PROXY',
               'https_proxy', 'http_proxy', 'all_proxy', 'no_proxy')


def write_json(path, value):
    temporary = path.with_suffix(path.suffix + '.pending')
    temporary.write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n')
    temporary.replace(path)


def read_json(path):
    return json.loads(path.read_text())


def catalog_receipt():
    database = Path.home()/'.local/share/opencode/opencode.db'
    with sqlite3.connect('file:' + str(database) + '?mode=ro', uri=True) as connection:
        row = connection.execute("SELECT value FROM kv WHERE key='models-dev:catalog'").fetchone()
    if row is None:
        raise ValueError('A native cached model catalog is required')
    record = json.loads(row[0])
    body = record['body']
    if isinstance(body, str):
        body = json.loads(body)
    selected = body['opencode-go']['models']['deepseek-v4.1-flash']
    return {'source': 'native models-dev:catalog', 'updated_at': record['updatedAt'],
            'catalog_digest': record['digest'], 'model': selected}


def runtime_identity(config, binary):
    data = tomllib.loads(config.read_text())
    commands = {row['command'] for row in data['components'].values()}
    components = {name: corpus.digest(binary.parent/name) for name in sorted(commands)}
    model_tree = binary.parent.parent/'share/opencode-runtime'
    files = corpus.manifest(model_tree)
    encoded = json.dumps(files, sort_keys=True, separators=(',', ':')).encode()
    return {'components': components, 'model_runtime_sha256': hashlib.sha256(encoded).hexdigest(),
            'model_runtime_files': len(files)}


def initialize(args):
    if args.tool_preflight is None:
        raise ValueError('A successful no-inference tool preflight is required before paid admission')
    preflight_path = args.tool_preflight.resolve()
    preflight = read_json(preflight_path)
    languages = preflight.get('languages', [])
    if (preflight.get('passed') is not True or preflight.get('paid_model_requests') != 0
            or {row.get('language') for row in languages} != {'python', 'go', 'java', 'rust', 'javascript', 'cpp'}
            or len(languages) != 6
            or any(row.get('passed') is not True or row.get('tool_calls') != 2
                   or row.get('tool_results') != 2
                   or row.get('cache_survives_separate_exec_commands') is not True
                   or row.get('tmpfs_isolated_per_tool') is not True for row in languages)):
        raise ValueError('Tool preflight must certify two isolated calls for all six languages')
    case_root = args.cases.resolve()
    source = read_json(case_root/'manifest.json')
    cases = []
    for row in source['cases']:
        case = case_root/row['id']
        if corpus.digest(case/'case.json') != row['case_sha256']:
            raise ValueError('Case changed before admission: ' + row['id'])
        info = read_json(case/'case.json')
        controls = control_receipt(case)
        cases.append({**row, 'exercise': info['exercise'],
                      'evaluable': controls.get('evaluable', True),
                      'evaluation_reason': controls.get('reason'),
                      'baseline_pass': controls['starter']['passed'],
                      'control_sha256': corpus.digest(DATA/'controls'/(row['id'] + '.json')),
                      'prompt_sha256': corpus.digest(case/'prompt.txt')})
    if len(cases) != 225:
        raise ValueError('The primary cohort must contain all 225 cases')
    order = [row['id'] for row in cases]
    random.Random(20261116).shuffle(order)
    record = {'format': 1, 'agents': list(AGENTS), 'case_root': str(case_root),
              'cases': cases, 'case_order': order, 'source_revision': source['source_rev'],
              'source_manifest_sha256': corpus.digest(case_root/'manifest.json'),
              'model_catalog': catalog_receipt(),
              'toolchain_setup_sha256': corpus.digest(grader.TOOLS/'receipt.json'),
              'toolchain_versions': read_json(grader.TOOLS/'receipt.json')['versions'],
              'tool_environment_preflight': {'path': str(preflight_path),
                                             'sha256': corpus.digest(preflight_path),
                                             'receipt': preflight},
              'upstream_methodology': read_json(DATA/'upstream-methodology/provenance.json'),
              'conditions': {'model': MODEL, 'effort': EFFORT, 'maximum_agent_turns': 2,
                             'agent_seconds_per_case': args.timeout,
                             'grader_timeout_seconds': 180, 'max_output_tokens': None,
                             'subagents': False, 'official_tests_visible_to_agents': False,
                             'external_plugins': False, 'grader_network': False,
                             'agent_network': 'unshared namespace; model endpoints only',
                             'network_allowlist': ['opencode.ai', 'models.opencode.ai'],
                             'soft_budget_usd': args.budget,
                             'cost_basis': 'catalog equivalent; not a Go billing receipt'},
              'base_config': str(CORE/'configs/agent.toml'),
              'benchmark_instruction_prefix': NETWORK_RULE,
              'base_config_sha256': corpus.digest(CORE/'configs/agent.toml'),
              'proteus_runtime': runtime_identity(CORE/'configs/agent.toml', BINARIES['proteus']),
              'stand_sources': {name: corpus.digest(HERE/name) for name in
                                ('polyglot_run.py', 'polyglot_corpus.py', 'polyglot_grader.py',
                                 'adapters.py', 'proteus_native.py', 'open_code_native.py',
                                 'polyglot_network.py')},
              'binaries': {name: {'path': str(path), 'sha256': corpus.digest(path)}
                           for name, path in BINARIES.items()}}
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    (args.output/'trials').mkdir()
    write_json(args.output/'run.json', record)
    print(args.output/'run.json', flush=True)


def cost_projection(metrics, prices):
    """Canonical input/output already include cache/reasoning: never add twice."""
    usage = metrics.get('provider_usage') or {}
    partial = metrics.get('partial_reported_usage_sums') or {}
    fields = ('input_tokens', 'cached_input_tokens', 'output_tokens')
    complete = all(type(usage.get(key)) is int for key in fields)
    if complete:
        inp, cached, output = (usage[key] for key in fields)
        if not 0 <= cached <= inp:
            raise ValueError('Invalid inclusive input/cache accounting')
        estimate = ((inp-cached)*prices['input'] + cached*prices['cache_read']
                    + output*prices['output'])/1_000_000
        return estimate, estimate, True
    observed = {key: usage.get(key) if type(usage.get(key)) is int else partial.get(key)
                for key in fields}
    # An unfinished request can be absent from the journal. Unknown input is
    # not priced as zero and cached input is never called uncached input.
    lower = ((observed.get('cached_input_tokens') or 0)*prices['cache_read']
             + (observed.get('output_tokens') or 0)*prices['output'])/1_000_000
    return None, lower, False


def sandbox(command, mounts, workspace, runtime, extra_env):
    argv = runner.sandbox_command(command, mounts, workspace, runtime)
    position = argv.index('--')
    settings = [item for pair in extra_env.items() for item in ('--setenv', *pair)]
    return [*argv[:position], *settings, *argv[position:]]


def restricted_sandbox(command, mounts, workspace, runtime, extra_env, socket_path):
    wrapped = ['/usr/bin/python3', '/runtime/polyglot_network.py', 'inside',
               '--socket', '/runtime/model-network.sock', '--', *command]
    network_mounts = [(HERE/'polyglot_network.py', '/runtime/polyglot_network.py'),
                      (socket_path, '/runtime/model-network.sock')]
    argv = sandbox(wrapped, mounts+network_mounts, workspace, runtime, extra_env)
    argv.insert(1, '--unshare-net')
    return argv


def toolchain_environment_override(runtime, environment):
    """Give Proteus shell tools the same prepared compiler env as native OpenCode."""
    path = runtime/'configs/eval.json'
    data = read_json(path)
    names = sorted(set(environment)-{'HOME', 'PATH'})
    for component in data['components'].values():
        if 'opencode' in component.get('exports', {}).get('model', {}):
            component['env_allowlist'] = sorted(set(component.get('env_allowlist', [])) | {'NODE_USE_ENV_PROXY'})
        if 'shell_tools' not in component.get('exports', {}).get('tool', {}):
            continue
        component['env_allowlist'] = sorted(set(component.get('env_allowlist', [])) | set(names))
    write_json(path, data)
    selection = read_json(runtime/'model-selection.json')
    selection['effective_config_sha256'] = corpus.digest(path)
    write_json(runtime/'model-selection.json', selection)
    overrides = read_json(runtime/'profile-overrides.json')
    overrides['benchmark_toolchain_environment'] = names
    overrides['model_network_environment'] = ['NODE_USE_ENV_PROXY']
    write_json(runtime/'profile-overrides.json', overrides)


def feedback(grade):
    return ('The official tests rejected the previous implementation. Fix the accepted '
            'implementation files using the test feedback below. Do not change the public '
            'API or fetch existing solutions. Only the declared files are submitted.\n\n'
            + grade.get('output', '')
            + '\n\nReturn a completed implementation for another official test run.\n')


def control_receipt(case):
    info = read_json(case/'case.json')
    if corpus.manifest(case/'repo') != info['repo_hashes']:
        raise ValueError('Starter tree differs from the admitted case: ' + case.name)
    path = DATA/'controls'/(case.name + '.json')
    receipt = read_json(path)
    if receipt.get('case_sha256') != corpus.digest(case/'case.json'):
        raise ValueError('Controls refer to a different case: ' + case.name)
    if not receipt.get('evaluable', True):
        return receipt
    if (receipt['starter']['passed'] and not receipt.get('starter_pass_allowed')) or not receipt['reference']['passed']:
        raise ValueError('Uncertified task: ' + case.name)
    return receipt


def classify_failure(report):
    if report.get('timed_out'):
        return 'agent_deadline', False
    if report.get('pipeline_ok'):
        return (None if report.get('passed') else 'solution_rejected'), False
    metrics = report.get('metrics', {})
    evidence = {'failures': [row.get('failure') for row in metrics.get('failures', [])
                             if row.get('kind') == 'model'],
                'errors': metrics.get('errors', [])}
    def provider_status(value):
        if isinstance(value, dict):
            for key, item in value.items():
                if key in ('status', 'statusCode', 'status_code') and item in (429, 502, 503, '429', '502', '503'):
                    return True
                if provider_status(item):
                    return True
        elif isinstance(value, list):
            return any(provider_status(item) for item in value)
        return False
    failures = json.dumps(evidence)
    diagnostic = failures + ' ' + (report.get('pipeline_error') or '')
    if provider_status(evidence) or any(word in diagnostic for word in ('HTTP 429', 'HTTP 502', 'HTTP 503',
                                           'ECONNRESET', 'UND_ERR_SOCKET', 'SocketError',
                                           'rate limit', 'quota exceeded')):
        return 'infrastructure_provider_transport_or_quota', True
    return 'harness_pipeline_failure', False


def run_case(root, frozen, agent, row):
    case = Path(frozen['case_root'])/row['id']
    output = root/'trials'/agent/row['id']
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    runtime, workspace = output/'runtime', output/'workspace'
    runtime.mkdir(mode=0o700)
    shutil.copytree(case/'repo', workspace, symlinks=True)
    subprocess.run(['git', 'init', '-q', str(workspace)], check=True)
    conditions = frozen['conditions']
    args = SimpleNamespace(agent=agent, model=MODEL, reasoning=EFFORT,
                           binary=Path(frozen['binaries'][agent]['path']),
                           base_config=Path(frozen['base_config']), timeout=conditions['agent_seconds_per_case'],
                           disable_subagents=True, auth_file=None, case_dir=case)
    info = read_json(case/'case.json')
    report = {'format': 1, 'agent': agent, 'case': row['id'], 'exercise': row['exercise'],
              'language': row['language'], 'model': MODEL, 'reasoning': EFFORT,
              'conditions': conditions, 'prompt_sha256': row['prompt_sha256'],
              'case_sha256': row['case_sha256'], 'binary': frozen['binaries'][agent],
              'expected_turns': 1, 'turns': [], 'grades': [], 'passed': False,
              'first_pass': False, 'pipeline_ok': False, 'wall_seconds': 0.0,
              'returncode': None, 'timed_out': False, 'usage_complete': False,
              'infrastructure_failure': False}
    started = time.monotonic()
    session = None
    environment = {key: os.environ[key] for key in PROXY_NAMES if key in os.environ}
    environment['PATH'] = '/usr/bin:/bin'
    try:
        report['controls'] = control_receipt(case)
        if corpus.digest(DATA/'controls'/(row['id'] + '.json')) != row['control_sha256']:
            raise ValueError('Case controls differ from the frozen cohort')
        if corpus.digest(case/'case.json') != row['case_sha256'] or corpus.digest(case/'prompt.txt') != row['prompt_sha256']:
            raise ValueError('Case/prompt differs from the frozen cohort')
        if corpus.manifest(workspace) != info['repo_hashes']:
            raise ValueError('Copied starter differs from the admitted input')
        inner, mounts = adapters.prepare(args, runtime)
        extra_mounts, extra_env = grader.agent_runtime(runtime, row['language'])
        if agent == 'proteus':
            toolchain_environment_override(runtime, extra_env)
        mounts += extra_mounts
        prompt = frozen.get('benchmark_instruction_prefix', '') + (case/'prompt.txt').read_text()
        report['source_prompt_sha256'] = report['prompt_sha256']
        report['prompt_sha256'] = hashlib.sha256(prompt.encode()).hexdigest()
        if agent == 'opencode':
            inner[inner.index('--prompt') + 1] = prompt
        else:
            inner[-1] = prompt
        (output/'prompt.txt').write_text(prompt)
        for number in (1, 2):
            command = list(inner)
            if number == 2:
                if agent == 'opencode':
                    command[command.index('--prompt') + 1] = prompt
                    command += ['--session', session]
                else:
                    command[-1:] = ['--resume-session', session, prompt]
            remaining = conditions['agent_seconds_per_case'] - report['wall_seconds']
            if remaining <= 0:
                report.update(timed_out=True, returncode=124)
                break
            status = runner.execute(restricted_sandbox(command, mounts, workspace, runtime, extra_env,
                                                       root/'model-network.sock'),
                                    output, remaining, environment, append=number > 1)
            report['wall_seconds'] += status['wall_seconds']
            report.update(returncode=status['returncode'], timed_out=status['timed_out'])
            report['expected_turns'] = number
            metrics = adapters.metrics(agent, output)
            handle, observed = runner.observed_session(agent, runtime)
            continuity = observed == number and (session is None or handle == session)
            session = handle
            report.update(metrics=metrics, continuity_verified=continuity)
            runner.check_pipeline(report)
            with GRADE_LOCK:
                grade = corpus.grade(case, workspace, output/f'grading-{number}.json')
            turn = {**status, 'number': number, 'metrics': metrics, 'grade': grade,
                    'pipeline_ok': report['pipeline_ok'], 'continuity_verified': continuity,
                    'observed_user_turns': observed,
                    'prompt_sha256': hashlib.sha256(prompt.encode()).hexdigest()}
            report['turns'].append(turn)
            report['grades'].append(grade)
            if number == 1:
                report['first_pass'] = bool(report['pipeline_ok'] and grade['passed'])
            report['passed'] = bool(report['pipeline_ok'] and grade['passed'])
            write_json(output/f'turn-{number}.json', turn)
            if report['passed'] or not report['pipeline_ok']:
                break
            prompt = feedback(grade)
            (output/'feedback.txt').write_text(prompt)
    except Exception as error:
        report['pipeline_error'] = str(error)
        report['pipeline_ok'] = report['passed'] = False
        if agent == 'opencode':
            database = runtime/'home/.local/share/opencode/opencode.db'
            if database.exists() and (runtime/'native/registry.json').exists():
                try:
                    open_code_native.save_usage(runtime/'native', database, after_logout=False)
                except Exception as partial_error:
                    report['partial_usage_error'] = type(partial_error).__name__
        try:
            report['metrics'] = adapters.metrics(agent, output)
        except Exception as metrics_error:
            report['metrics_error'] = type(metrics_error).__name__
    finally:
        if len(report['grades']) < report['expected_turns']:
            try:
                with GRADE_LOCK:
                    snapshot = corpus.grade(case, workspace, output/'grading-final-snapshot.json')
                report['grades'].append({**snapshot, 'phase': 'interrupted_final_snapshot'})
            except Exception as grade_error:
                report['grading_error'] = type(grade_error).__name__
        runner.remove_auth(runtime)
        report['auth_removed'] = True
        report['trial_wall_seconds'] = time.monotonic()-started
        report['final_solution_sha256'] = {name: corpus.digest(workspace/name)
                                           for name in info['solution_files']
                                           if grader.safe_solution(workspace, name)}
        report['usage_cost_usd'], report['observed_cost_usd_lower_bound'], complete = cost_projection(
            report.get('metrics', {}), frozen['model_catalog']['model']['cost'])
        report['usage_complete'] = bool(complete and report.get('pipeline_ok'))
        report['failure_kind'], report['infrastructure_failure'] = classify_failure(report)
        write_json(output/'result.json', report)
    return report


def run(args):
    root = args.output.resolve()
    frozen = read_json(root/'run.json')
    if any(corpus.digest(Path(row['path'])) != row['sha256'] for row in frozen['binaries'].values()):
        raise ValueError('An admitted native binary changed')
    if corpus.digest(Path(frozen['base_config'])) != frozen['base_config_sha256']:
        raise ValueError('The admitted Proteus assembly changed')
    if runtime_identity(Path(frozen['base_config']), Path(frozen['binaries']['proteus']['path'])) != frozen['proteus_runtime']:
        raise ValueError('An admitted component or native model runtime changed')
    if corpus.digest(Path(frozen['case_root'])/'manifest.json') != frozen['source_manifest_sha256']:
        raise ValueError('The full source cohort manifest changed')
    if corpus.digest(grader.TOOLS/'receipt.json') != frozen['toolchain_setup_sha256']:
        raise ValueError('The admitted toolchain setup receipt changed')
    rows = {row['id']: row for row in frozen['cases']}
    order = args.only or frozen['case_order']
    if any(name not in rows for name in order):
        raise ValueError('Requested case outside frozen cohort')
    jobs, pending_controls = [], []
    for index, name in enumerate(order):
        if not rows[name].get('evaluable', True):
            continue
        case = Path(frozen['case_root'])/name
        try:
            control_receipt(case)
        except (FileNotFoundError, KeyError, ValueError):
            pending_controls.append(name)
            continue
        arms = AGENTS if index % 2 == 0 else tuple(reversed(AGENTS))
        for agent in arms:
            if (root/'trials'/agent/name/'result.json').exists():
                continue
            if (root/'trials'/agent/name).exists():
                raise ValueError('Interrupted trial requires explicit audit: ' + name + '/' + agent)
            jobs.append((agent, rows[name]))
    existing = [read_json(path) for path in (root/'trials').glob('*/*/result.json')]
    spent = sum(row.get('observed_cost_usd_lower_bound', 0) for row in existing)
    budget = frozen['conditions']['soft_budget_usd']
    queued = iter(jobs)
    completed = len(existing)
    planned = len(frozen['agents']) * sum(row.get('evaluable', True) for row in rows.values())
    unmetered = [row['case'] + '/' + row['agent'] for row in existing
                 if not row.get('usage_complete')]
    from polyglot_network import ModelNetwork
    with ModelNetwork(root/'model-network.sock') as network, \
            concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        active = {}
        while True:
            while len(active) < args.jobs and spent < budget and not unmetered:
                try:
                    agent, row = next(queued)
                except StopIteration:
                    break
                future = pool.submit(run_case, root, frozen, agent, row)
                active[future] = (agent, row['id'])
            if not active:
                break
            done, _ = concurrent.futures.wait(active, return_when=concurrent.futures.FIRST_COMPLETED)
            for future in done:
                agent, case_id = active.pop(future)
                result = future.result()
                spent += result.get('observed_cost_usd_lower_bound', 0)
                if not result.get('usage_complete'):
                    unmetered.append(case_id + '/' + agent)
                completed += 1
                print(json.dumps({'completed': completed, 'agent': agent, 'case': case_id,
                                  'first_pass': result['first_pass'], 'passed': result['passed'],
                                  'failure_kind': result['failure_kind'],
                                  'agent_seconds': round(result['wall_seconds'], 2),
                                  'cost_usd_lower_bound': round(spent, 6)}), flush=True)
                write_json(root/'progress.json', {'completed': completed,
                           'planned': planned, 'cost_usd_lower_bound': spent,
                           'pending_controls': pending_controls,
                           'admission_blocked_unmetered': unmetered,
                           'soft_budget_reached': spent >= budget})
    write_json(root/'network.json', network.snapshot())
    print(json.dumps({'completed': completed, 'planned': planned,
                      'cost_usd_lower_bound': spent, 'pending_controls': len(pending_controls),
                      'admission_blocked_unmetered': unmetered,
                      'soft_budget_reached': spent >= budget}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['init', 'run'])
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--cases', type=Path, default=DATA/'cases')
    parser.add_argument('--timeout', type=int, default=1200)
    parser.add_argument('--budget', type=float, default=20.0)
    parser.add_argument('--jobs', type=int, choices=[1, 2, 3, 4], default=2)
    parser.add_argument('--only', nargs='+')
    parser.add_argument('--tool-preflight', type=Path)
    args = parser.parse_args()
    if args.action == 'init':
        initialize(args)
    else:
        run(args)


if __name__ == '__main__':
    main()

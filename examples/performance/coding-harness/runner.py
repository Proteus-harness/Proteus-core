#!/usr/bin/env python3
"""Run one native coding harness in an isolated copy of a public task."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import shutil
import subprocess
import time

import adapters
import open_code_native


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def tree_manifest(root):
    result = {}
    for path in sorted(root.rglob('*')):
        if '.git' in path.relative_to(root).parts:
            continue
        if path.is_symlink():
            result[str(path.relative_to(root))] = {'symlink': os.readlink(path)}
        elif path.is_file():
            result[str(path.relative_to(root))] = digest(path)
    return result


def sandbox_command(command, mounts, workspace, runtime):
    result = ['bwrap', '--die-with-parent', '--new-session', '--unshare-pid',
              '--ro-bind', '/usr', '/usr', '--ro-bind', '/etc', '/etc',
              '--symlink', 'usr/bin', '/bin', '--symlink', 'usr/lib', '/lib',
              '--symlink', 'usr/lib64', '/lib64', '--proc', '/proc', '--dev', '/dev',
              '--tmpfs', '/tmp', '--bind', str(workspace), '/workspace',
              '--bind', str(runtime), '/state', '--chdir', '/workspace',
              '--setenv', 'HOME', '/state/home', '--setenv', 'CODEX_HOME', '/state/home/.codex',
              '--setenv', 'PI_CODING_AGENT_DIR', '/state/home/.pi/agent',
              '--setenv', 'XDG_CONFIG_HOME', '/state/home/.config',
              '--setenv', 'XDG_DATA_HOME', '/state/home/.local/share',
              '--setenv', 'XDG_CACHE_HOME', '/state/home/.cache',
              '--setenv', 'XDG_STATE_HOME', '/state/home/.local/state',
              '--setenv', 'PATH', '/runtime/bin:/usr/local/bin:/usr/bin:/bin',
              '--setenv', 'TMPDIR', '/tmp']
    for mount in mounts:
        source, destination = mount[:2]
        writable = len(mount) == 3 and mount[2]
        result.extend(['--bind' if writable else '--ro-bind', str(source), destination])
    return [*result, '--', *command]


def execute(command, output, timeout, environment, *, append=False):
    started = time.monotonic()
    timed_out = False
    mode = 'a' if append else 'w'
    with (output/'stdout.jsonl').open(mode) as stdout, (output/'stderr.log').open(mode) as stderr:
        process = subprocess.Popen(command, env=environment, stdin=subprocess.DEVNULL,
                                   stdout=stdout, stderr=stderr, start_new_session=True)
        try:
            returncode = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(process.pid, signal.SIGTERM)
            try:
                returncode = process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                returncode = process.wait()
    return {'returncode': returncode, 'timed_out': timed_out,
            'wall_seconds': time.monotonic()-started}


def remove_auth(runtime):
    for auth in [runtime/'home/.codex/auth.json', runtime/'home/.pi/agent/auth.json',
                 runtime/'home/.local/share/opencode/auth.json',
                 runtime/'home/.config/Proteus-agent/secrets/chatgpt.json']:
        auth.unlink(missing_ok=True)
    (runtime/'opencode-auth-export.json').unlink(missing_ok=True)
    database = runtime/'home/.local/share/opencode/opencode.db'
    if database.exists():
        # Metrics and safe provenance were extracted before this finally block.
        # Remove the disposable credential database even on interrupted logout.
        for suffix in ['', '-wal', '-shm']:
            Path(str(database)+suffix).unlink(missing_ok=True)


def check_pipeline(report):
    metrics = report.get('metrics', {})
    if report['agent'] == 'proteus':
        models = [json.loads(model)['model'] for model in metrics.get('model_refs', [])]
        efforts = [json.loads(effort).get('effort') for effort in metrics.get('reasoning_refs', [])]
        settled = metrics.get('settlements') == [{'status':'success','error':None}] * report.get('expected_turns', 1)
        complete = metrics.get('model_requests', 0) == metrics.get('model_responses')
    else:
        models = metrics.get('models', [])
        efforts = metrics.get('reasoning_efforts', [])
        settled = metrics.get('settled', False)
        complete = not metrics.get('errors')
    expected_model = report['model'].split('/', 1)[-1]
    report['model_verified'] = bool(models) and all(model == expected_model for model in models)
    if report['agent'] == 'opencode' or '/' in report['model']:
        provider = report['model'].split('/', 1)[0] if '/' in report['model'] else 'openai'
        report['model_verified'] = report['model_verified'] and metrics.get('providers') == [provider]
    report['reasoning_verified'] = bool(efforts) and all(effort == report['reasoning'] for effort in efforts)
    report['pipeline_ok'] = bool(report.get('returncode') == 0 and not report.get('timed_out')
                                 and settled and complete and not metrics.get('error')
                                 and report['model_verified'] and report['reasoning_verified']
                                 and (report.get('expected_turns', 1) == 1 or report.get('continuity_verified')))


def observed_session(agent, runtime):
    """Return a resume handle and count admitted user turns from durable state."""
    if agent == 'pi':
        paths = list((runtime/'pi-sessions').glob('*.jsonl'))
        if len(paths) == 1:
            rows = adapters.json_lines(paths[0])
            turns = sum(row.get('type') == 'message' and row.get('message', {}).get('role') == 'user'
                        for row in rows)
            return '/state/pi-sessions/' + paths[0].name, turns
    elif agent == 'proteus':
        paths = list((runtime/'sessions').rglob('journal.jsonl'))
        if len(paths) == 1:
            turns = sum(row.get('kind') == 'turn_opened' for row in adapters.json_lines(paths[0]))
            return '/runtime/sessions/' + str(paths[0].parent.relative_to(runtime/'sessions')), turns
    elif agent == 'opencode':
        path = runtime/'native/usage.json'
        if path.is_file():
            data = json.loads(path.read_text())
            if len(data['sessions']) == 1:
                session = data['sessions'][0]['id']
                return session, data['user_turns'].get(session, 0)
    raise ValueError(f'{agent} did not leave exactly one resumable session')


def run_turns(args, inner, mounts, environment, report, prompts):
    runtime, workspace = args.output/'runtime', args.output/'workspace'
    started = time.monotonic()
    session = None
    report['turns'] = []
    for index, prompt in enumerate(prompts):
        command = list(inner)
        if index:
            if args.agent == 'opencode':
                command[command.index('--prompt') + 1] = prompt
                command += ['--session', session]
            else:
                flag = '--resume-session' if args.agent == 'proteus' else '--session'
                command[-1:] = [flag, session, prompt]
        remaining = args.timeout - (time.monotonic() - started)
        if remaining <= 0:
            report.update(returncode=124, timed_out=True)
            break
        status = execute(sandbox_command(command, mounts, workspace, runtime), args.output, remaining,
                         environment, append=bool(index))
        receipt = {**status, 'number': index+1, 'prompt_sha256': hashlib.sha256(prompt.encode()).hexdigest()}
        report['turns'].append(receipt)
        report.update(status)
        if status['returncode'] or status['timed_out']:
            break
        if len(prompts) > 1:
            handle, count = observed_session(args.agent, runtime)
            receipt.update(session=handle, observed_user_turns=count)
            if count != index+1 or (session is not None and session != handle):
                raise ValueError('follow-up session identity or journal length differs')
            session = handle
    report['wall_seconds'] = time.monotonic() - started
    report['continuity_verified'] = bool(session and len(report['turns']) == len(prompts)
                                         and report['turns'][-1].get('observed_user_turns') == len(prompts))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case-dir', type=Path, required=True)
    parser.add_argument('--agent', choices=['pi','codex','proteus','opencode'], required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--model', default='opencode-go/deepseek-v4.1-flash')
    parser.add_argument('--reasoning', default='high')
    parser.add_argument('--base-config', type=Path, default=adapters.CORE/'configs/codex.config.toml')
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--auth-file', type=Path, help='native auth file, copied privately; never serialized')
    parser.add_argument('--timeout', type=int, default=600)
    parser.add_argument('--pass-env', action='append', default=[], help='explicit environment variable name to pass')
    parser.add_argument('--prepare-only', action='store_true', help='stage and expose command without a model call')
    args = parser.parse_args()
    args.case_dir = args.case_dir.expanduser().resolve()
    args.output = args.output.expanduser().resolve()
    args.base_config = args.base_config.expanduser().resolve()
    if not (args.case_dir/'prompt.txt').is_file() or not (args.case_dir/'repo').is_dir():
        parser.error('case-dir requires prompt.txt and pristine repo/')
    if not 1 <= args.timeout <= 3600:
        parser.error('timeout must be 1..3600 seconds')
    if not shutil.which('bwrap'):
        parser.error('bubblewrap is required to contain native agent commands')
    default_binary = (adapters.CORE.parent/'Proteus-desktop/build/Proteus/bin/proteus'
                      if args.agent == 'proteus' else shutil.which(args.agent))
    if args.binary is None and default_binary is None:
        parser.error(f'{args.agent} is not installed; supply --binary')
    args.binary = Path(args.binary or default_binary).expanduser().resolve()
    if not args.binary.is_file():
        parser.error('binary does not exist')
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    runtime = args.output/'runtime'
    runtime.mkdir(mode=0o700)
    workspace = args.output/'workspace'
    shutil.copytree(args.case_dir/'repo', workspace, symlinks=True)
    # Nested repository data must be standalone, never a worktree pointer back to host data.
    if (workspace/'.git').is_file():
        parser.error('fixture repo must not contain a linked-worktree .git file')
    if not (workspace/'.git').exists():
        subprocess.run(['git','init','-q',str(workspace)],check=True)
    before = tree_manifest(workspace)
    prompt = (args.case_dir/'prompt.txt').read_bytes()
    (args.output/'prompt.txt').write_bytes(prompt)
    prompts = [prompt.decode()]
    if (args.case_dir/'followups.json').is_file():
        extra = json.loads((args.case_dir/'followups.json').read_text())
        if args.agent == 'codex' or not isinstance(extra, list) or not all(isinstance(t, str) and t.strip() for t in extra):
            parser.error('followups require supported agent and nonempty text turns')
        prompts.extend(extra)
    try:
        inner, mounts = adapters.prepare(args,runtime)
    except (ValueError, OSError) as error:
        remove_auth(runtime)
        parser.error(str(error))
    command = sandbox_command(inner,mounts,workspace,runtime)
    environment = {name:value for name,value in os.environ.items()
                   if name in ['LANG','LC_ALL','HTTPS_PROXY','HTTP_PROXY','ALL_PROXY','NO_PROXY',
                               'https_proxy','http_proxy','all_proxy','no_proxy',*args.pass_env]}
    environment['PATH'] = '/usr/bin:/bin'
    report = {'format':1,'agent':args.agent,'model':args.model,'reasoning':args.reasoning,
              'binary':str(args.binary),'binary_sha256':digest(args.binary),
              'prompt_sha256':hashlib.sha256(prompt).hexdigest(),
              'fixture_files_sha256':before,'command':command,
              'environment_names':sorted(environment),'isolation':'bubblewrap-system-readonly-workspace-state-write',
              'auth_present':True,'prepared_only':args.prepare_only, 'expected_turns':len(prompts)}
    if len(prompts) > 1:
        report['prompt_sha256'] = hashlib.sha256(json.dumps(prompts, ensure_ascii=False).encode()).hexdigest()
    try:
        if not args.prepare_only:
            started = time.monotonic()
            try:
                run_turns(args, inner, mounts, environment, report, prompts)
            except (OSError, ValueError, KeyError) as error:
                report.update(returncode=1, pipeline_error=str(error), wall_seconds=time.monotonic()-started)
            if args.agent == 'opencode' and not (runtime/'native/usage.json').is_file():
                database = runtime/'home/.local/share/opencode/opencode.db'
                if database.is_file() and (runtime/'native/registry.json').is_file():
                    try:
                        open_code_native.save_usage(runtime/'native', database, after_logout=False)
                    except (OSError, ValueError, open_code_native.sqlite3.Error) as error:
                        report['partial_usage_error'] = str(error)
            try:
                report['metrics'] = adapters.metrics(args.agent,args.output)
            except (ValueError,KeyError,OSError) as error:
                report['metrics'] = {'error':str(error)}
            report['final_files_sha256'] = tree_manifest(workspace)
            check_pipeline(report)
    finally:
        remove_auth(runtime)
        report['auth_removed'] = True
    (args.output/'result.json').write_text(json.dumps(report,indent=2)+'\n')
    print(args.output/'result.json')
    return 0 if args.prepare_only or report['pipeline_ok'] else 1


if __name__ == '__main__':
    raise SystemExit(main())

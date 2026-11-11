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


def execute(command, output, timeout, environment):
    started = time.monotonic()
    timed_out = False
    with (output/'stdout.jsonl').open('w') as stdout, (output/'stderr.log').open('w') as stderr:
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
        settled = metrics.get('settlements') == [{'status':'success','error':None}]
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
                                 and report['model_verified'] and report['reasoning_verified'])


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
              'auth_present':True,'prepared_only':args.prepare_only}
    try:
        if not args.prepare_only:
            report.update(execute(command,args.output,args.timeout,environment))
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

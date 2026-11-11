#!/usr/bin/env python3
"""Replay a finished Proteus benchmark with no network or writable source paths."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess


def manifest(root):
    result = {}
    for path in sorted(root.rglob('*')):
        name = str(path.relative_to(root))
        if path.is_symlink():
            result[name] = {'symlink': os.readlink(path)}
        elif path.is_file():
            with path.open('rb') as stream:
                result[name] = hashlib.file_digest(stream, 'sha256').hexdigest()
    return result


def credential_bytes():
    """Read only local native Go credentials; never export or serialize them."""
    database = Path.home()/'.local/share/opencode/opencode.db'
    if not database.is_file():
        return []
    with sqlite3.connect(database.as_uri()+'?mode=ro', uri=True) as connection:
        values = connection.execute(
            "SELECT value FROM credential WHERE integration_id='opencode-go' AND active=1"
        ).fetchall()
    return [value['key'].encode() for row in values
            if (value := json.loads(row[0])).get('type') == 'key' and value.get('key')]


def audit(source, output, turn_id=None, module_config=None):
    source = source.resolve()
    output = output.resolve()
    report = json.loads((source/'result.json').read_text())
    if report.get('agent') != 'proteus' or report.get('auth_removed') is not True:
        raise ValueError('Require a finished Proteus run whose credentials were removed')
    journals = list((source/'runtime/sessions').rglob('journal.jsonl'))
    if len(journals) != 1:
        raise ValueError('Expected one session journal in the benchmark output')
    journal = journals[0]
    auth_files = [path for path in (source/'runtime').rglob('*')
                  if path.is_file() and (path.name in ['auth.json', 'opencode-auth-export.json',
                                                       'openai-auth-export.json', 'nativeauth.json']
                                         or path.name.endswith('.key'))]
    needles = credential_bytes()
    leaked = [str(path.relative_to(source)) for path in source.rglob('*')
              if path.is_file() and not path.is_symlink()
              and any(needle in path.read_bytes() for needle in needles)]
    if auth_files or leaked:
        raise ValueError('Source still contains authentication files or credential bytes')
    output.mkdir(parents=True, exist_ok=False)
    config = {'include': ['/runtime/configs/eval.json'],
              'event_log': {'path': '/tmp/replay-events.jsonl'}}
    if module_config is not None:
        config['module_config'] = module_config
    config_path = output/'replay.config.json'
    config_path.write_text(json.dumps(config, indent=2)+'\n')
    original = report['command']
    if original[0] != 'bwrap':
        raise ValueError('Replay audit requires an existing bubblewrap benchmark command')
    command = ['--ro-bind' if item == '--bind' else item for item in original[:original.index('--')]]
    command.insert(1, '--unshare-net')
    command += ['--ro-bind', str(config_path), '/replay.config.json', '--',
                '/runtime/bin/proteus', '--config', '/replay.config.json', 'replay', 'workflow',
                '/state/'+str(journal.parent.relative_to(source/'runtime')), '--json']
    if turn_id:
        command += ['--turn-id', turn_id]
    before = manifest(source/'workspace')
    session_before = manifest(journal.parent)
    journal_before = hashlib.sha256(journal.read_bytes()).hexdigest()
    timed_out = False
    with (output/'stdout.json').open('w') as stdout, (output/'stderr.log').open('w') as stderr:
        process = subprocess.Popen(command, env={'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8'},
                                   stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                   start_new_session=True)
        try:
            returncode = process.wait(timeout=120)
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(process.pid, signal.SIGKILL)
            returncode = process.wait()
    try:
        replay = json.loads((output/'stdout.json').read_text())
    except ValueError:
        replay = {}
    result = {
        'source': str(source), 'command': command, 'returncode': returncode, 'timed_out': timed_out,
        'network': 'bubblewrap --unshare-net', 'source_mounts': 'read-only',
        'credentials': {'auth_files': 0, 'active_go_keys_checked': len(needles), 'key_byte_matches': 0},
        'journal_sha256_before': journal_before,
        'journal_sha256_after': hashlib.sha256(journal.read_bytes()).hexdigest(),
        'workspace_unchanged': before == manifest(source/'workspace'),
        'session_files_unchanged': session_before == manifest(journal.parent),
        'matched': replay.get('comparison', {}).get('matched', False),
        'model_exchanges': replay.get('model_exchanges'), 'tool_calls': replay.get('tool_calls'),
        'issues': replay.get('comparison', {}).get('issues'),
    }
    result['audit_ok'] = bool(not timed_out and returncode == 0 and result['matched']
                              and result['workspace_unchanged'] and result['session_files_unchanged']
                              and result['journal_sha256_before'] == result['journal_sha256_after'])
    (output/'audit.json').write_text(json.dumps(result, indent=2)+'\n')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--turn-id')
    parser.add_argument('--module-config', type=Path, help='explicit compatibility factory overrides')
    args = parser.parse_args()
    result = audit(args.source, args.output, args.turn_id,
                   json.loads(args.module_config.read_text()) if args.module_config else None)
    print(json.dumps({key: result[key] for key in ['audit_ok', 'matched', 'model_exchanges', 'tool_calls', 'issues']}, indent=2))
    return 0 if result['audit_ok'] else 1


if __name__ == '__main__':
    raise SystemExit(main())

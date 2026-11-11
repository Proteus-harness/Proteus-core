#!/usr/bin/env python3
"""Run a native OpenCode V2 turn against its own prewarmed private server."""
import argparse
import json
import os
from pathlib import Path
import secrets
import selectors
import sqlite3
import subprocess
import sys
import time



def active_export(data, provider):
    """Select exactly one active native credential; never stage other accounts."""
    if not isinstance(data, list):
        raise ValueError('Native credential export must be an array')
    entries = [entry for entry in data if isinstance(entry, dict)
               and entry.get('integrationID') == provider and entry.get('active') is True]
    if len(entries) != 1:
        raise ValueError(f'Expected exactly one active native credential for {provider}')
    return entries


def fresh_go_export(binary=None):
    binary = binary or Path.home()/'.opencode/bin/opencode'
    try:
        response = subprocess.run([str(binary), 'auth', 'export', 'opencode-go'],
                                  capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ValueError('Cannot obtain fresh native OpenCode Go credential export') from error
    if response.returncode:
        raise ValueError('Native OpenCode Go credential export failed')
    try:
        entries = active_export(json.loads(response.stdout), 'opencode-go')
    except json.JSONDecodeError as error:
        raise ValueError('Native OpenCode Go export is not valid JSON') from error
    value = entries[0].get('value', {})
    if value.get('type') != 'key' or not isinstance(value.get('key'), str) or not value['key']:
        raise ValueError('Active native OpenCode Go export requires an API key')
    return entries


def write_private_json(destination, value):
    destination = Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, 'w') as stream:
        json.dump(value, stream)

def prepare_auth_export(source, destination, provider):
    """Stage supported external credentials without putting secrets in reports."""
    data = json.loads(Path(source).expanduser().read_text())
    if isinstance(data, list):
        entries = active_export(data, provider)
    else:
        entry = data.get('openai-codex', data.get('openai')) if provider == 'openai' else data.get(provider)
        if not isinstance(entry, dict):
            raise ValueError(f'Authentication file has no credentials for {provider}')
        if entry.get('type') == 'oauth' and provider == 'openai':
            value = {
                'type': 'oauth', 'methodID': 'chatgpt-headless',
                **{key: entry[key] for key in ['access', 'refresh', 'expires']},
                'metadata': {'accountID': entry['accountId']},
            }
        elif entry.get('type') == 'api':
            value = {'type': 'key', 'key': entry['key']}
        else:
            raise ValueError(f'Unsupported authentication format for native OpenCode {provider}')
        entries = [{'id': 'cred_coding_eval', 'integrationID': provider,
                    'label': 'coding-eval', 'active': True, 'value': value}]
    if not entries:
        raise ValueError(f'Authentication export has no credentials for {provider}')
    write_private_json(destination, entries)


def api_json(binary, environment, url, operation, location=False):
    command = [str(binary), 'api', '--server', url, operation]
    if location:
        command.extend(['--param', 'location[directory]=' + str(Path.cwd())])
    response = subprocess.run(command, env=environment, capture_output=True, text=True, timeout=20)
    if response.returncode:
        raise RuntimeError(f'OpenCode metadata request failed for {operation}')
    return json.loads(response.stdout)


def save_registry(binary, environment, url, model, state):
    provider, name = model.split('#', 1)[0].split('/', 1)
    models = api_json(binary, environment, url, 'model.list', location=True)['data']
    selected = next(item for item in models if item['id'] == name and item['providerID'] == provider)
    plugins = api_json(binary, environment, url, 'plugin.list', location=True)
    plugin_rows = plugins.get('data', plugins) if isinstance(plugins, dict) else plugins
    config_sources = api_json(binary, environment, url, 'config.get', location=True)
    bindings = {}
    for source in config_sources:
        info = source.get('info', {})
        if 'model' in info:
            bindings['main'] = info['model']
        title = info.get('agents', {}).get('title', {})
        if 'model' in title:
            bindings['title'] = title['model']
    if provider == 'opencode-go':
        expected = {'providerID': provider, 'model': name, 'variant': model.split('#', 1)[1]}
        if bindings.get('main') != expected or bindings.get('title') != expected:
            raise RuntimeError('Flash main/title model binding is not explicitly verified; refusing generation')
    proof = {
        'model_bindings': bindings,
        'auxiliary_model_verified': provider == 'opencode-go',
        'model': {key: selected.get(key) for key in ['id', 'modelID', 'providerID', 'name', 'limit', 'compatibility']},
        'variants': [variant['id'] for variant in selected.get('variants', [])],
        'plugins': [{key: plugin.get(key) for key in ['id', 'version', 'source']} for plugin in plugin_rows],
    }
    (state/'registry.json').write_text(json.dumps(proof, indent=2) + '\n')


def save_usage(state):
    database = Path(os.environ.get('XDG_DATA_HOME', str(Path.home()/'.local/share')))/'opencode/opencode.db'
    with sqlite3.connect(database) as connection:
        connection.row_factory = sqlite3.Row
        sessions = [dict(row) for row in connection.execute(
            'SELECT id, model, tokens_input, tokens_output, tokens_reasoning, '
            'tokens_cache_read, tokens_cache_write, idle_outcome FROM session_v2'
        )]
        for session in sessions:
            session['model'] = json.loads(session['model']) if session['model'] else None
        messages = []
        for row in connection.execute("SELECT session_id, data FROM session_message WHERE type='assistant'"):
            message = json.loads(row['data'])
            messages.append({'session_id': row['session_id'], **{
                key: message.get(key) for key in ['model', 'tokens', 'cost', 'finish', 'time']
            }, 'tools': [part['name'] for part in message.get('content', []) if part.get('type') == 'tool']})
        credentials = connection.execute('SELECT count(*) FROM credential').fetchone()[0]
        (state/'usage.json').write_text(json.dumps({
            'sessions': sessions, 'assistant_messages': messages, 'credentials_remaining': credentials,
        }, indent=2)+'\n')
        if credentials:
            raise RuntimeError('OpenCode private credential cleanup left stored credentials')
        # Native logout deletes logical credentials. Compact the disposable DB
        # after server exit so deleted credential bytes do not survive in pages.
        connection.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        connection.execute('VACUUM')
        connection.execute('PRAGMA wal_checkpoint(TRUNCATE)')


def start_server(binary, environment, log, timeout):
    process = subprocess.Popen(
        [str(binary), 'serve', '--stdio', '--hostname', '127.0.0.1', '--port', '0'],
        env=environment, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log,
        text=True, bufsize=1,
    )
    try:
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            if not selector.select(timeout):
                raise RuntimeError('OpenCode private server did not become ready')
        line = process.stdout.readline()
        ready = json.loads(line)
        url = ready['url']
        if not isinstance(url, str) or not url.startswith('http://127.0.0.1:'):
            raise RuntimeError('OpenCode returned an unexpected private server address')
        return process, url
    except BaseException:
        process.kill()
        process.wait()
        raise


def wait_model(binary, environment, url, model, timeout):
    deadline = time.monotonic() + timeout
    wanted = model.split('#', 1)[0]
    while time.monotonic() < deadline:
        response = subprocess.run(
            [str(binary), 'models', '--server', url], env=environment,
            capture_output=True, text=True, timeout=min(10, max(1, deadline-time.monotonic())),
        )
        if response.returncode == 0 and wanted in response.stdout.splitlines():
            return
        time.sleep(0.25)
    raise RuntimeError(f'OpenCode private server has no available model {wanted}')


def run(binary, model, prompt, state, startup_timeout=60, auth_export=None):
    state.mkdir(parents=True, exist_ok=True)
    state.chmod(0o700)
    environment = os.environ.copy()
    environment['OPENCODE_PASSWORD'] = secrets.token_urlsafe(32)
    environment['OPENCODE_CONFIG_PROJECT_DISABLE'] = '1'
    with (state/'opencode-server.stderr.log').open('w') as log:
        process, url = start_server(binary, environment, log, startup_timeout)
        entries = []
        try:
            if auth_export is not None:
                imported = subprocess.run(
                    [str(binary), 'auth', 'import', '--server', url, str(auth_export)],
                    env=environment, capture_output=True, text=True, timeout=20,
                )
                if imported.returncode:
                    raise RuntimeError('OpenCode private credential import failed')
            authentication = subprocess.run(
                [str(binary), 'auth', 'list', '--server', url, '--format', 'json'],
                env=environment, capture_output=True, text=True, timeout=20,
            )
            if authentication.returncode:
                raise RuntimeError('OpenCode private authentication initialization failed')
            entries = json.loads(authentication.stdout)
            (state/'auth-metadata.json').write_text(json.dumps([
                {'integration': entry['id'], 'connections': len(entry.get('connections', []))}
                for entry in entries
            ], indent=2))
            wait_model(binary, environment, url, model, startup_timeout)
            save_registry(binary, environment, url, model, state)
            return subprocess.run(
                [str(binary), 'run', '--server', url, '--auto', '--format', 'json',
                 '--model', model, prompt],
                env=environment, stdin=subprocess.DEVNULL,
            ).returncode
        finally:
            for entry in entries:
                for credential in entry.get('connections', []):
                    if credential.get('type') == 'credential':
                        removed = subprocess.run(
                            [str(binary), 'auth', 'logout', '--server', url, entry['id'], credential['id']],
                            env=environment, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=20,
                        )
                        if removed.returncode:
                            print('OpenCode private credential cleanup failed', file=sys.stderr)
            process.stdin.close()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            save_usage(state)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--model', required=True)
    parser.add_argument('--prompt', required=True)
    parser.add_argument('--state', type=Path, required=True)
    parser.add_argument('--startup-timeout', type=int, default=60)
    parser.add_argument('--auth-export', type=Path)
    args = parser.parse_args()
    try:
        return run(args.binary, args.model, args.prompt, args.state, args.startup_timeout, args.auth_export)
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f'OpenCode startup failed: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())

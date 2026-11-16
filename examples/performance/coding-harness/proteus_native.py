"""Disposable preparation for the self-contained native OpenCode model assembly."""
import hashlib
import json
import os
from contextlib import closing
import sqlite3
import tomllib


def selected_model(path):
    """Recognize the explicit named-model native assembly, not a profile basename."""
    data = tomllib.loads(path.read_text())
    alias = data.get('model')
    model = data.get('models', {}).get(alias) if isinstance(alias, str) else None
    if not isinstance(model, dict) or model.get('provider') != 'opencode':
        return None
    if data.get('include'):
        raise ValueError('Native benchmark assembly must be self-contained')
    return data, alias


def stage_credential(home, entries):
    """Only the selected native export enters an isolated, private credential DB."""
    path = home/'.local/share/opencode/opencode.db'
    path.parent.mkdir(parents=True, mode=0o700)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.close(descriptor)
    entry = entries[0]
    with closing(sqlite3.connect(path)) as database, database:
        database.execute('CREATE TABLE credential(id TEXT PRIMARY KEY, integration_id TEXT, '
                         'value TEXT, active INTEGER, time_created INTEGER)')
        database.execute('INSERT INTO credential VALUES(?,?,?,?,?)',
                         (entry['id'], entry['integrationID'], json.dumps(entry['value']), 1, 1))


def prepare(args, runtime, selected, entries):
    data, alias = selected
    if not args.model.startswith('opencode-go/') or not entries:
        raise ValueError('Native named-model benchmark currently requires OpenCode Go')
    binding = data.get('module_config', {}).get('model', {}).get('opencode', {})
    if binding.get('provider') != 'opencode-go':
        raise ValueError('Native OpenCode model binding must select opencode-go')
    data['models'][alias].update(model=args.model.split('/', 1)[1], effort=args.reasoning)
    # Keep slots/prompts/hooks/tool schemas. Unattended policy is justified by
    # the enclosing OS sandbox, exactly as in the older benchmark adapters.
    data['module_config'].setdefault('policy', {})['codex_policy'] = {
        'allow': data.get('tools', {}).get('enabled', []),
        'allow_sandboxed': [], 'ask_before': [],
    }
    data.setdefault('tools', {})['mcp_servers'] = []
    data.setdefault('runtime', {}).update(workflow_timeout_ms=args.timeout * 1000,
                                          model_timeout_ms=args.timeout * 1000)
    data['event_log'] = {'path': '/state/events.jsonl'}
    overrides = {'unattended_policy': True, 'mcp_servers': [],
                 'disable_subagents': bool(getattr(args, 'disable_subagents', False))}
    if overrides['disable_subagents']:
        data['agent_control'] = {'surface': 'none', 'roles': []}
    configs = runtime/'configs'
    configs.mkdir()
    config = configs/'eval.json'
    config.write_text(json.dumps(data, indent=2)+'\n')
    (runtime/'sessions').mkdir()
    stage_credential(runtime/'home', entries)
    (runtime/'model-selection.json').write_text(json.dumps({
        'provider': binding['provider'], 'model': args.model.split('/', 1)[1],
        'reasoning': args.reasoning, 'adapter': 'opencode', 'alias': alias,
        'source_config_sha256': hashlib.sha256(args.base_config.read_bytes()).hexdigest(),
        'effective_config_sha256': hashlib.sha256(config.read_bytes()).hexdigest(),
        'provider_evidence': 'module-config binding; upstream response does not attest provider identity',
    }, indent=2)+'\n')
    (runtime/'profile-overrides.json').write_text(json.dumps(overrides, indent=2)+'\n')
    portable = args.binary.parent.parent
    mounts = [(args.binary.parent, '/runtime/bin'),
              (portable/'share/opencode-runtime', '/runtime/share/opencode-runtime'),
              (configs, '/runtime/configs'), (runtime/'sessions', '/runtime/sessions', True)]
    return ['/runtime/bin/'+args.binary.name, '--config', '/runtime/configs/eval.json',
            '--cwd', '/workspace', (args.case_dir/'prompt.txt').read_text()], mounts


def bound_provider(runtime, model_refs):
    """Prove configured upstream selection alongside observed logical adapter IDs."""
    path = runtime/'model-selection.json'
    if not path.exists():
        return None
    selection = json.loads(path.read_text())
    if selection.get('adapter') != 'opencode':
        return None
    config = runtime/'configs/eval.json'
    if hashlib.sha256(config.read_bytes()).hexdigest() != selection['effective_config_sha256']:
        raise ValueError('Prepared native model configuration changed during evaluation')
    actual_provider = json.loads(config.read_text())['module_config']['model']['opencode']['provider']
    if selection['provider'] != actual_provider:
        raise ValueError('Native upstream provider receipt differs from bound module configuration')
    refs = [json.loads(ref) for ref in model_refs]
    if not refs or any(ref['provider'] != 'opencode' for ref in refs):
        raise ValueError('Observed model adapter differs from native OpenCode binding')
    return selection

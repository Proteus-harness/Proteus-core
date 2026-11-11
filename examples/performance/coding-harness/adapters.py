"""Native CLI commands and reported usage for disposable coding evaluations."""
import importlib.util
import json
from pathlib import Path
import shutil
import tomllib
from collections import Counter

HERE = Path(__file__).resolve().parent
CORE = HERE.parents[2]


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def copy_auth(source, destination):
    source = Path(source).expanduser()
    if not source.is_file():
        raise ValueError(f"required authentication file missing: {source}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(0o600)


def copy_openai_auth(source, destination, pi=False):
    data = json.loads(Path(source).expanduser().read_text())
    entry = data.get('openai-codex') if pi else data.get('openai')
    if entry is None:
        entry = data.get('openai') if pi else data.get('openai-codex')
    if not isinstance(entry, dict) or entry.get('type') != 'oauth':
        raise ValueError('OpenAI OAuth entry required in the supplied auth file')
    entry = {key: entry[key] for key in ['type','access','refresh','expires','accountId'] if key in entry}
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps({'openai-codex' if pi else 'openai': entry}))
    destination.chmod(0o600)


def selected_tools(path):
    data = tomllib.loads(path.read_text())
    names = None
    includes = data.get('include', [])
    if isinstance(includes, str):
        includes = [includes]
    for include in includes:
        included = selected_tools(path.parent/include)
        if included is not None:
            names = included
    return data.get('tools', {}).get('enabled', names)


def prepare(args, runtime):
    """Return inner command and readonly mounts; auth values never enter results."""
    home = runtime / 'home'
    home.mkdir()
    mounts = []
    prompt = (args.case_dir / 'prompt.txt').read_text()
    go = args.model.startswith('opencode-go/')
    model = args.model.split('/', 1)[-1]
    if go and (model != 'deepseek-v4.1-flash' or args.reasoning not in ('low', 'high', 'max')):
        raise ValueError('Pinned Flash requires exact deepseek-v4.1-flash and low/high/max effort')
    if go and args.agent == 'codex':
        raise ValueError('Native Codex unavailable: OpenCode Go Flash exposes Chat Completions; Responses endpoint rejected probe HTTP 403')
    native = load_module('open_code_native', HERE/'open_code_native.py')
    go_auth = native.fresh_go_export() if go else None
    if go_auth:
        (runtime/'model-selection.json').write_text(json.dumps({
            'provider': 'opencode-go', 'model': model, 'reasoning': args.reasoning,
            'catalog': '@earendil-works/pi-ai (locked Pi 1.5.2)',
            'api': 'openai-completions', 'contextWindow': 1000000, 'maxTokens': 384000,
            'baseUrl': 'https://opencode.ai/inference/go/openai/v1',
        }, indent=2)+'\n')

    def stage_pi_go():
        native.write_private_json(home/'.pi/agent/auth.json', {
            'opencode-go': {'type': 'api_key', 'key': go_auth[0]['value']['key']},
        })
    if args.agent == 'proteus':
        portable_bin = args.binary.parent
        mounts.append((portable_bin, '/runtime/bin'))
        if args.base_config.name == 'pi.config.toml' or go:
            mounts.append((portable_bin.parent/'share/pi-runtime','/runtime/share/pi-runtime'))
        if args.base_config.name == 'opencode.config.toml':
            mounts.append((portable_bin.parent/'share/opencode-runtime', '/runtime/share/opencode-runtime'))
        configs = runtime / 'configs'
        shutil.copytree(CORE / 'configs', configs)
        try:
            profile = args.base_config.relative_to(CORE / 'configs')
        except ValueError as error:
            raise ValueError('--base-config must be under the Core configs directory') from error
        config = {
            'include': ['/runtime/configs/' + str(profile)],
            'active_provider': 'chatgpt',
            'providers': {'chatgpt': {'model': model,
                                      'reasoning': {'effort': args.reasoning, 'summary': True}}},
            'tools': {'mcp_servers': []},
            'runtime': {'workflow_timeout_ms': args.timeout * 1000,
                        'model_timeout_ms': args.timeout * 1000},
            'event_log': {'path': '/state/events.jsonl'},
        }
        if args.base_config.name == 'opencode.config.toml' and not go:
            config['include'].append('/runtime/configs/fragments/openai-chatgpt.toml')
        # An outer OS sandbox permits unattended tools while containing writes.
        # Keep each profile's tool selection and workflow/prompt/hooks intact.
        config['module_config'] = {'policy': {'codex_policy': {
            'allow': selected_tools(args.base_config), 'allow_sandboxed': [], 'ask_before': [],
        }}}
        if go:
            config['providers']['chatgpt']['provider'] = 'pi_codex'
            config['module_config']['model'] = {'pi_codex': {'implementation': 'opencode-go'}}
            if args.base_config.name == 'pi.config.toml':
                config['module_config']['workflow'] = {'pi_loop': {'catalog_provider': 'opencode-go'}}
                config['module_config']['hook'] = {'pi_compact': {'catalog_provider': 'opencode-go'}}
            if args.base_config.name != 'pi.config.toml':
                config['components'] = {'pi-benchmark-model': {
                    'command': 'proteus-pi-module',
                    'env_allowlist': ['HOME', 'PI_CODING_AGENT_DIR', 'PATH', 'HTTP_PROXY', 'HTTPS_PROXY',
                                      'ALL_PROXY', 'NO_PROXY', 'http_proxy', 'https_proxy', 'all_proxy', 'no_proxy'],
                    'exports': {'model': {'pi_codex': {}}},
                }}
        if go and args.base_config.name == 'codex.config.toml':
            config['module_config']['tool'] = {'codex_patch': {'freeform': False}}
            (runtime/'profile-overrides.json').write_text(json.dumps({
                'codex_patch.freeform': False,
                'reason': 'Flash transport supports function tools only; original patch engine retained',
                'native_codex_pair_available': False,
            }, indent=2)+'\n')
        (configs / 'eval.json').write_text(json.dumps(config, indent=2) + '\n')
        mounts.append((configs, '/runtime/configs'))
        sessions = runtime/'sessions'
        sessions.mkdir()
        (configs/'sessions').mkdir()
        mounts.append((sessions, '/runtime/sessions', True))
        if go:
            stage_pi_go()
        elif args.base_config.name == 'pi.config.toml':
            copy_openai_auth(args.auth_file or Path.home()/'.pi/agent/auth.json', home/'.pi/agent/auth.json', pi=True)
        else:
            copy_auth(args.auth_file or Path.home()/'.config/Proteus-agent/secrets/chatgpt.json',
                      home/'.config/Proteus-agent/secrets/chatgpt.json')
        return ['/runtime/bin/proteus', '--config', '/runtime/configs/eval.json',
                '--cwd', '/workspace', prompt], mounts
    if args.agent == 'codex':
        copy_auth(args.auth_file or Path.home()/'.codex/auth.json', home/'.codex/auth.json')
        return [str(args.binary), '-a', 'never', 'exec', '--ignore-user-config',
                '--ignore-rules', '--sandbox', 'danger-full-access', '--json',
                '-C', '/workspace', '--model', args.model,
                '-c', 'model_reasoning_effort=' + json.dumps(args.reasoning),
                '-c', 'web_search="disabled"',
                '-c', 'features.apps=false', '-c', 'features.plugins=false',
                '-c', 'features.remote_plugin=false',
                '-c', 'features.plugin_discovery_prefetch=false',
                '-c', 'features.hooks=false', prompt], mounts
    if args.agent == 'pi':
        if go:
            stage_pi_go()
        else:
            copy_openai_auth(args.auth_file or Path.home()/'.pi/agent/auth.json',
                             home/'.pi/agent/auth.json', pi=True)
        binary = args.binary
        if str(binary).startswith('/usr/'):
            executable = [str(binary)]
        else:
            node_modules = next((p for p in binary.parents if p.name == 'node_modules'), None)
            if node_modules is None:
                raise ValueError('Pi binary must resolve to its CLI file under node_modules or /usr')
            mounts.append((node_modules, '/runtime/node_modules'))
            executable = ['/usr/bin/node', '/runtime/node_modules/' + str(binary.relative_to(node_modules))]
        return [*executable, '--provider', 'opencode-go' if go else 'openai-codex', '--model', model,
                '--thinking', args.reasoning, '--mode', 'json', '--no-extensions',
                '--no-skills', '--no-prompt-templates', '--no-themes', '--no-mcp',
                '--tools', 'read,bash,edit,write', '--session-dir', '/state/pi-sessions',
                '-p', prompt], mounts
    native = load_module('open_code_native', HERE/'open_code_native.py')
    model = args.model if '/' in args.model else 'openai/' + args.model
    provider = model.split('/', 1)[0]
    if go:
        native.write_private_json(runtime/'opencode-auth-export.json', go_auth)
    else:
        native.prepare_auth_export(
            args.auth_file or Path.home()/'.pi/agent/auth.json',
            runtime/'opencode-auth-export.json', provider,
        )
    mounts.append((args.binary, '/runtime/opencode'))
    mounts.append((HERE/'open_code_native.py', '/runtime/open_code_native.py'))
    config = home / '.config/opencode/opencode.json'
    config.parent.mkdir(parents=True)
    config.write_text(json.dumps({'$schema': 'https://opencode.ai/config.json',
                                 'plugins': [], 'mcp': {'servers': {}},
                                 **({'model': f'{model}#{args.reasoning}', 'small_model': model,
                                     'agents': {'title': {'model': f'{model}#{args.reasoning}'}}} if go else {}),
                                 'permissions': [{'action': '*', 'resource': '*', 'effect': 'allow'}]}))
    return ['/usr/bin/python3', '-B', '/runtime/open_code_native.py',
            '--binary', '/runtime/opencode', '--model', f'{model}#{args.reasoning}',
            '--prompt', prompt, '--state', '/state/native',
            '--auth-export', '/state/opencode-auth-export.json'], mounts


def json_lines(path):
    with path.open(errors='replace') as stream:
        for line in stream:
            try:
                value = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(value, dict):
                yield value


def metrics(agent, output):
    """Preserve native usage and distinguish missing categories from zero."""
    if agent == 'proteus':
        journals = list((output/'runtime/sessions').glob('**/journal.jsonl'))
        if len(journals) != 1:
            return {'error': f'expected one canonical journal, found {len(journals)}'}
        module = load_module('coding_usage', HERE.parent/'code-mode/measurements.py')
        result = module.measure(journals[0].parent)
        journal = module._journal
        providers = set()
        for record, payload in journal.records(journals[0].parent):
            if record['kind'] == 'model_response_recorded':
                for message in payload.get('outcome', {}).get('response', {}).get('messages', []):
                    native = (message.get('metadata') or {}).get('pi_native', {})
                    provider = native.get('header', native).get('provider')
                    if provider:
                        providers.add(provider)
        result['providers'] = sorted(providers)
        if (output/'runtime/model-selection.json').exists():
            result['model_selection'] = json.loads((output/'runtime/model-selection.json').read_text())
        if (output/'runtime/profile-overrides.json').exists():
            result['profile_overrides'] = json.loads((output/'runtime/profile-overrides.json').read_text())
        # recorded sandbox paths must be translated only for opening artifacts,
        # never for canonical hash or replay comparisons.
        return result
    events = list(json_lines(output/'stdout.jsonl'))
    if agent == 'codex':
        completed = [e for e in events if e.get('type') == 'turn.completed']
        rows = [e['usage'] for e in completed if isinstance(e.get('usage'), dict)]
        contexts = [row['payload'] for path in (output/'runtime/home/.codex/sessions').rglob('*.jsonl')
                    for row in json_lines(path) if row.get('type') == 'turn_context']
        actions = [e['item'] for e in events if e.get('type') == 'item.completed'
                   and isinstance(e.get('item'), dict)
                   and e['item'].get('type') in ('command_execution','file_change','mcp_tool_call','tool_call')]
        return {'native_usage': rows, 'model_requests': None,
                'tool_names': dict(Counter(a.get('tool', a['type']) for a in actions)),
                'all_tool_calls': len(actions),
                'models': sorted({c['model'] for c in contexts}),
                'reasoning_efforts': sorted({c['effort'] for c in contexts if c.get('effort')}),
                'provider_usage': sum_usage(rows, {'input_tokens': 'input_tokens',
                                                  'cached_input_tokens': 'cached_input_tokens',
                                                  'output_tokens': 'output_tokens'}),
                'settled': bool(completed), 'events': len(events),
                'errors': [e for e in events if e.get('type') in ('error', 'turn.failed')]}
    if agent == 'pi':
        messages = [e['message'] for e in events if e.get('type') == 'message_end'
                    and isinstance(e.get('message'), dict) and e['message'].get('role') == 'assistant']
        rows = [m['usage'] for m in messages if isinstance(m.get('usage'), dict)]
        levels = [row['thinkingLevel'] for path in (output/'runtime/pi-sessions').glob('*.jsonl')
                  for row in json_lines(path) if row.get('type') == 'thinking_level_change']
        totals = sum_usage(rows, {'uncached_input_tokens': 'input', 'cached_input_tokens': 'cacheRead',
                                  'cache_creation_input_tokens': 'cacheWrite', 'output_tokens': 'output',
                                  'reasoning_output_tokens': 'reasoning'})
        parts = [totals.get(k) for k in ['uncached_input_tokens', 'cached_input_tokens',
                                       'cache_creation_input_tokens']]
        totals['input_tokens'] = sum(parts) if all(p is not None for p in parts) else None
        return {'native_usage': rows, 'provider_usage': totals, 'model_requests': len(messages),
                'tool_names': dict(Counter(p.get('name','unknown') for m in messages
                                          for p in m.get('content',[]) if p.get('type') == 'toolCall')),
                'all_tool_calls': sum(p.get('type') == 'toolCall' for m in messages for p in m.get('content',[])),
                'models': sorted({m.get('model', '') for m in messages}),
                'providers': sorted({m['provider'] for m in messages if m.get('provider')}),
                'reasoning_efforts': sorted(set(levels)),
                'settled': any(e.get('type') == 'agent_end' for e in events),
                'errors': [m.get('errorMessage') for m in messages if m.get('stopReason') in ('error','aborted')]}
    native = json.loads((output/'runtime/native/usage.json').read_text())
    rows = native['sessions']
    messages = native['assistant_messages']
    totals = sum_usage(rows, {'uncached_input_tokens': 'tokens_input',
                             'visible_output_tokens': 'tokens_output',
                             'cached_input_tokens': 'tokens_cache_read',
                             'cache_creation_input_tokens': 'tokens_cache_write',
                             'reasoning_output_tokens': 'tokens_reasoning'})
    parts = [totals.get(k) for k in ['uncached_input_tokens', 'cached_input_tokens', 'cache_creation_input_tokens']]
    totals['input_tokens'] = sum(parts) if all(p is not None for p in parts) else None
    output_parts = [totals.get('visible_output_tokens'), totals.get('reasoning_output_tokens')]
    totals['output_tokens'] = sum(output_parts) if all(p is not None for p in output_parts) else None
    tools = [tool for message in messages for tool in message['tools']]
    return {'native_usage': rows, 'provider_usage': totals, 'model_requests': None,
            'primary_model_requests': len(messages), 'usage_includes_auxiliary_requests': True,
            'tool_names': dict(Counter(tools)), 'all_tool_calls': len(tools),
            'models': sorted({message['model']['id'] for message in messages}),
            'providers': sorted({message['model']['providerID'] for message in messages}),
            'reasoning_efforts': sorted({message['model']['variant'] for message in messages}),
            'registry': json.loads((output/'runtime/native/registry.json').read_text()),
            'credentials_remaining': native['credentials_remaining'],
            'settled': bool(rows) and all(row['idle_outcome'] == 'succeeded' for row in rows),
            'errors': [e for e in events if e.get('type') == 'error']}


def sum_usage(rows, mapping):
    totals = {}
    for field, native in mapping.items():
        values = [r.get(native) for r in rows]
        totals[field] = sum(values) if values and all(type(v) is int for v in values) else None
    return totals

#!/usr/bin/env python3
"""Zero-paid installed-profile proof, not an inference benchmark or parity score.

Run real Codex/Pi/OpenCode workflows and tools with a deterministic model/v14
export. Credentials are removed before launch and the outer namespace has no
network. All file effects are confined to a fresh synthetic workspace.
"""
import argparse
import copy
import json
from pathlib import Path
import sys
import tomllib
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
PROFILES = ('codex', 'pi', 'opencode')
MARKER = 'PROFILE_TOOL_SMOKE_OK\n'
CALL_IDS = ('preflight-first', 'preflight-second')


def fixture_main():
    from polyglot_tool_preflight import response
    fixture = Path('/core/crates/proteus-core/tests/fixtures/process_model.py')
    namespace = {'__file__': str(fixture), '__name__': 'profile_tool_fixture'}
    source = fixture.read_text().rsplit('\nrun_component(', 1)[0]
    exec(compile(source, str(fixture), 'exec'), namespace)
    original = namespace['invoke']

    def invoke(context, method, params):
        if method != 'stream':
            return original(context, method, params)
        config = namespace['settings'][context.export['module_id']]
        request = params['request']
        with Path('/state/model-requests.jsonl').open('a') as captured:
            captured.write(json.dumps(request)+'\n')
        if request['model']['model'] != 'deepseek-v4.1-flash':
            raise namespace['ProtocolError']('Exact native catalog model ID changed')
        results = {part['payload']['ToolResult']['result']['call_id']:
                   part['payload']['ToolResult']['result']
                   for message in request['messages'] for part in message['parts']
                   if 'ToolResult' in part['payload']}
        step = 0
        for index, call_id in enumerate(CALL_IDS):
            if call_id not in results:
                break
            if not results[call_id]['ok']:
                raise namespace['ProtocolError']('Native tool failed: ' + str(results[call_id]))
            if (config['calls'][index]['name'] in ('read', 'exec_command')
                    and MARKER.strip() not in results[call_id]['output']):
                raise namespace['ProtocolError']('Native read did not return the fixture content')
            step += 1
        terminal = response(step, ['', ''])
        reply = terminal['terminal']['response']
        if step < 2:
            call = reply['tool_calls'][0]
            call.update(config['calls'][step])
            spec = next((tool for tool in request['tools'] if tool['name'] == call['name']), None)
            if spec is None:
                raise namespace['ProtocolError']('Probe tool is not model-visible')
            schema = spec['input_schema']
            if (not set(schema.get('required', [])).issubset(call['args'])
                    or not set(call['args']).issubset(schema.get('properties', {}))):
                raise namespace['ProtocolError']('Probe arguments do not match native schema')
        else:
            reply['messages'][0]['parts'][0]['payload']['Text']['text'] = 'PROFILE_TOOL_SMOKE_COMPLETE'
        return terminal

    namespace['run_component'](namespace['initialize'], invoke)


def tool_calls(profile):
    if profile == 'codex':
        return [
            {'name': 'apply_patch', 'args': {'patch':
             '*** Begin Patch\n*** Add File: /workspace/proof.txt\n+' + MARKER + '*** End Patch'}},
            {'name': 'exec_command', 'args': {'cmd': 'cat /workspace/proof.txt',
             'workdir': '/workspace', 'max_output_tokens': 1000, 'yield_time_ms': 1000}},
        ]
    return [{'name': 'read', 'args': {'path': '/workspace/probe.txt'}},
            {'name': 'write', 'args': {'path': '/workspace/proof.txt', 'content': MARKER}}]


def run_profile(root, prefix, profile, timeout):
    import adapters
    from polyglot_tool_preflight import descriptor
    import runner

    core = HERE.parents[2]
    output = root/profile
    runtime, workspace, case = output/'runtime', output/'workspace', output/'case'
    runtime.mkdir(parents=True, mode=0o700)
    workspace.mkdir()
    case.mkdir()
    (workspace/'probe.txt').write_text(MARKER)
    (case/'prompt.txt').write_text('Perform the deterministic native-tool smoke probe.\n')
    base = core/'configs'/f'{profile}.config.toml'
    source = tomllib.loads(base.read_text())
    args = SimpleNamespace(agent='proteus', model='opencode-go/deepseek-v4.1-flash',
                           reasoning='high', binary=prefix/'bin/proteus', base_config=base,
                           timeout=timeout, disable_subagents=True, auth_file=None, case_dir=case)
    try:
        inner, mounts = adapters.prepare(args, runtime)
        runner.remove_auth(runtime)
        config = runtime/'configs/eval.json'
        data = json.loads(config.read_text())
        # Undo the benchmark adapter's generic unattended policy. Keep the
        # profile's policy implementation and settings, admitting probe tools only.
        data['module_config']['policy'] = copy.deepcopy(source['module_config']['policy'])
        calls = tool_calls(profile)
        policy_override = None
        if data['modules']['policy'] == 'codex_policy':
            policy_override = {'allow': [call['name'] for call in calls],
                               'allow_sandboxed': [], 'ask_before': []}
            data['module_config']['policy']['codex_policy'] = policy_override
        for identity in list(data['components']):
            if data['components'][identity].get('exports', {}).get('model'):
                del data['components'][identity]
        data['components']['profile-tool-fixture'] = {
            'command': 'python3', 'args': ['-B', '/runtime/profile_tool_smoke.py', 'fixture'],
            'exports': {'model': {'preflight_fixture': {}}}}
        data['models'][data['model']]['provider'] = 'preflight_fixture'
        data['module_config']['model'] = {'preflight_fixture': {
            'descriptor': descriptor(), 'features': [], 'calls': calls}}
        config.write_text(json.dumps(data, indent=2)+'\n')
        mounts += [(core/'crates/proteus-core/tests/fixtures/process_model.py',
                    '/core/crates/proteus-core/tests/fixtures/process_model.py'),
                   (core/'examples/modules/component_runtime.py', '/core/examples/modules/component_runtime.py'),
                   (HERE/'polyglot_tool_preflight.py', '/runtime/polyglot_tool_preflight.py'),
                   (Path(__file__).resolve(), '/runtime/profile_tool_smoke.py')]
        if profile == 'pi':
            mounts.append((prefix/'share/pi-runtime', '/runtime/share/pi-runtime'))
        command = runner.sandbox_command(inner, mounts, workspace, runtime)
        command.insert(1, '--unshare-net')
        status = runner.execute(command, output, timeout, {'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8'})
        records = [json.loads(line) for path in (runtime/'sessions').rglob('journal.jsonl')
                   for line in path.read_text().splitlines()]
        results = [row['payload']['value']['result'] for row in records if row['kind'] == 'tool_result_recorded']
        requested = [row['payload']['value'] for row in records if row['kind'] == 'tool_call_recorded'
                     and row['payload']['value']['phase'].get('phase') == 'requested']
        settled = [row['payload']['value'] for row in records if row['kind'] == 'turn_settled']
        proof = workspace/'proof.txt'
        expected_ids = set(CALL_IDS)
        okay = (status['returncode'] == 0 and not status['timed_out']
                and len(requested) == 2 and len(results) == 2
                and {result['call_id'] for result in results} == expected_ids
                and all(result['ok'] for result in results)
                and proof.is_file() and proof.read_text() == MARKER
                and (workspace/'probe.txt').read_text() == MARKER
                and len(settled) == 1 and settled[0]['status'] == 'success'
                and settled[0]['error'] is None
                and settled[0]['output']['text'] == 'PROFILE_TOOL_SMOKE_COMPLETE')
        receipt = {'profile': profile, 'passed': okay, **status, 'paid_model_requests': 0,
                   'binary_sha256': runner.digest(args.binary), 'source_config_sha256': runner.digest(base),
                   'effective_config_sha256': runner.digest(config), 'slots': data['modules'],
                   'model': data['models'][data['model']],
                   'overrides': {'model_export': 'deterministic model/v14 fixture',
                                 'root_only': True, 'probe_policy': policy_override,
                                 'runtime_timeout_seconds': timeout, 'event_log': '/state/events.jsonl'},
                   'network': 'outer bubblewrap --unshare-net; staged credentials removed before execution',
                   'model_requests_sha256': runner.digest(runtime/'model-requests.jsonl')
                   if (runtime/'model-requests.jsonl').exists() else None,
                   'host_proof': proof.read_text() if proof.is_file() else None,
                   'tool_calls': requested, 'tool_results': results, 'settlements': settled}
    finally:
        runner.remove_auth(runtime)
    (output/'result.json').write_text(json.dumps(receipt, indent=2)+'\n')
    print(json.dumps({key: receipt[key] for key in ('profile', 'passed', 'wall_seconds')}), flush=True)
    return receipt


def main():
    if sys.argv[1:] == ['fixture']:
        fixture_main()
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', type=Path, default=HERE.parents[2].parent/'Proteus-desktop/build/Proteus-dot-165-166')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--profiles', nargs='+', choices=PROFILES, default=list(PROFILES))
    parser.add_argument('--timeout', type=int, default=120)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    rows = [run_profile(args.output.resolve(), args.prefix.resolve(), name, args.timeout) for name in args.profiles]
    receipt = {'passed': all(row['passed'] for row in rows), 'paid_model_requests': 0, 'profiles': rows}
    (args.output/'result.json').write_text(json.dumps(receipt, indent=2)+'\n')
    raise SystemExit(0 if receipt['passed'] else 1)


if __name__ == '__main__':
    main()

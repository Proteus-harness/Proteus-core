#!/usr/bin/env python3
"""No-inference compiler/cache proof through the real Codex tool workflow.

Only the model component is replaced by a deterministic model/v13 fixture.
Two separately sandboxed exec_command calls write/read a persistent cache
marker and run an offline compiler/dependency smoke test. No credentials remain
at execution time; the outer namespace has no network. This is tool environment
proof, not a model score or an authoritative exercise grading run.
"""
import argparse
import json
from pathlib import Path
import sys
import uuid
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
LANGUAGES = ('python', 'go', 'java', 'rust', 'javascript', 'cpp')
CALL_IDS = ('preflight-first', 'preflight-second')


def response(step, commands):
    if step < 2:
        call = {'id': CALL_IDS[step], 'name': 'exec_command',
                'args': {'cmd': commands[step], 'workdir': '/workspace',
                         'max_output_tokens': 3000, 'yield_time_ms': 30000},
                'surface': 'function', 'raw_arguments': None}
        payload, calls, reason = {'ToolCall': {'call': call}}, [call], 'ToolCalls'
    else:
        payload, calls, reason = {'Text': {'text': 'TOOL_PREFLIGHT_COMPLETE'}}, [], 'Stop'
    message = {'id': str(uuid.uuid4()), 'role': 'Assistant', 'phase': None,
               'parts': [{'part_id': str(uuid.uuid4()), 'provenance': 'model',
                          'scope': 'conversation', 'payload': payload}],
               'name': None, 'tool_call_id': None, 'metadata': None}
    return {'event_count': 0, 'terminal': {'kind': 'response', 'response': {
        'messages': [message], 'tool_calls': calls, 'finish_reason': reason,
        'end_turn': not calls, 'usage': None, 'provider_metadata': {'fixture': True}}}}


def fixture_main():
    fixture = Path('/core/crates/proteus-core/tests/fixtures/process_model.py')
    namespace = {'__file__': str(fixture), '__name__': 'preflight_fixture'}
    source = fixture.read_text().rsplit('\nrun_component(', 1)[0]
    exec(compile(source, str(fixture), 'exec'), namespace)
    original = namespace['invoke']

    def invoke(context, method, params):
        if method != 'stream':
            return original(context, method, params)
        config = namespace['settings'][context.export['module_id']]
        results = {part['payload']['ToolResult']['result']['call_id']: part['payload']['ToolResult']['result']
                   for message in params['request']['messages'] for part in message['parts']
                   if 'ToolResult' in part['payload']}
        step = 0
        for index, call_id in enumerate(CALL_IDS):
            if call_id not in results:
                break
            result = results[call_id]
            if not result['ok'] or f'PREFLIGHT_OK_{index+1}' not in result['output']:
                raise namespace['ProtocolError']('Compiler/cache smoke failed: ' + result['output'][-3000:])
            step += 1
        return response(step, config['commands'])

    namespace['run_component'](namespace['initialize'], invoke)


def smoke(language):
    """Synthetic sources exercise the same prepared dependency stores as tasks."""
    if language == 'python':
        return {'smoke_test.py': 'def test_runtime():\n    assert sum([1, 2]) == 3\n'}, \
            'python -m pytest --version\npython -m pytest -q smoke_test.py', '/workspace/.benchmark-cache/python'
    if language == 'go':
        return {'go.mod': 'module example.com/preflight\n\ngo 1.22\n',
                'smoke_test.go': 'package preflight\nimport "testing"\nfunc TestSmoke(t *testing.T) { if 1+1 != 2 { t.Fatal("sum") } }\n'}, \
            'go test -count=1 ./...', '"$GOCACHE"'
    if language == 'rust':
        return {'Cargo.toml': '[package]\nname="tool-preflight"\nversion="0.1.0"\nedition="2021"\n[dependencies]\nanyhow="=1.0.104"\ntime="=0.3.55"\n',
                'src/lib.rs': '#[test]\nfn dependencies_work() { let result: anyhow::Result<()> = Ok(()); assert!(result.is_ok()); assert_eq!(time::OffsetDateTime::UNIX_EPOCH.unix_timestamp(), 0); }\n'}, \
            'cargo test --offline', '"$CARGO_HOME"'
    if language == 'javascript':
        return {'package.json': '{"name":"tool-preflight","private":true}\n',
                'smoke.test.js': 'test("prepared dependencies", () => { expect(require("@babel/core").version).toBeTruthy(); expect(1+1).toBe(2); });\n'}, \
            'test ! -w node_modules\nnode --version\nnpm --version\nnode node_modules/jest/bin/jest.js --runInBand smoke.test.js --config \'{"testEnvironment":"node"}\'', '"$NPM_CONFIG_CACHE"'
    if language == 'java':
        return {'settings.gradle': 'rootProject.name = "tool-preflight"\n',
                'build.gradle': 'plugins { id "java" }\nrepositories { mavenCentral() }\ndependencies { testImplementation platform("org.junit:junit-bom:5.10.0"); testImplementation "org.junit.jupiter:junit-jupiter" }\ntest { useJUnitPlatform() }\n',
                'src/test/java/SmokeTest.java': 'import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.assertEquals;\nclass SmokeTest { @Test void dependenciesWork() { assertEquals(2, 1+1); } }\n'}, \
            'gradle --offline --no-daemon --console=plain test --rerun-tasks', '"$GRADLE_USER_HOME"'
    if language == 'cpp':
        return {'CMakeLists.txt': 'cmake_minimum_required(VERSION 3.5)\nproject(tool_preflight CXX)\nfind_package(Boost 1.58 REQUIRED COMPONENTS date_time)\nforeach(name gigasecond meetup)\nadd_executable(${name} smoke.cpp)\ntarget_link_libraries(${name} PRIVATE Boost::date_time)\nendforeach()\n',
                'smoke.cpp': '#include <boost/date_time/posix_time/posix_time.hpp>\n#include <boost/date_time/gregorian/gregorian.hpp>\nint main() { using namespace boost::gregorian; using namespace boost::posix_time; auto stamp = ptime(date(2011,4,25)) + seconds(1000000000); auto monday = nth_day_of_the_week_in_month(nth_day_of_the_week_in_month::first, Monday, May).get_date(2026); return stamp.date().year() == 2043 && monday.day() == 4 ? 0 : 1; }\n'}, \
            'cmake -S . -B build -DBoost_NO_BOOST_CMAKE=ON -DCMAKE_POLICY_VERSION_MINIMUM=3.5\ncmake --build build --parallel 1\n./build/gigasecond\n./build/meetup', '/workspace/.benchmark-cache/cpp'
    raise ValueError('Unsupported language: ' + language)


def commands(language):
    files, compile_command, cache = smoke(language)
    common = f'set -eu\ncache={cache}\n'
    first = common + ('mkdir -p "$cache"\nprintf persistent > "$cache/.proteus-preflight"\n'
                      'printf ephemeral > /tmp/proteus-preflight-ephemeral\n')
    second = common + ('test "$(cat "$cache/.proteus-preflight")" = persistent\n'
                       'test ! -e /tmp/proteus-preflight-ephemeral\n')
    return files, [first+compile_command+'\nprintf PREFLIGHT_OK_1\n',
                   second+compile_command+'\nprintf PREFLIGHT_OK_2\n']


def descriptor():
    features = ('tools', 'parallel_tool_calls', 'streaming', 'system_role', 'developer_role', 'reasoning_config')
    names = (*features, 'freeform_tools', 'json_schema', 'cache_hints', 'image_input', 'file_input')
    return {'adapter_id': 'preflight_fixture', 'hosted_tools': [], 'capabilities': {
        **{'supports_'+name: name in features for name in names},
        'provider_hosted_tools': [], 'max_input_tokens': 1000000, 'max_output_tokens': 384000}}


def run_language(root, language, timeout):
    import adapters
    import polyglot_grader as grader
    import polyglot_run as driver
    import runner

    output = root/language
    runtime, workspace, case = output/'runtime', output/'workspace', output/'case'
    runtime.mkdir(parents=True, mode=0o700)
    workspace.mkdir()
    case.mkdir()
    (case/'prompt.txt').write_text('Run the deterministic compiler/cache tool preflight.\n')
    files, tool_commands = commands(language)
    for name, text in files.items():
        path = workspace/name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    args = SimpleNamespace(agent='proteus', model=driver.MODEL, reasoning=driver.EFFORT,
                           binary=driver.BINARIES['proteus'], base_config=driver.CORE/'configs/config.toml',
                           timeout=timeout, disable_subagents=True, auth_file=None, case_dir=case)
    try:
        inner, mounts = adapters.prepare(args, runtime)
        runner.remove_auth(runtime)
        data = driver.read_json(runtime/'configs/eval.json')
        original_slots = data['modules'].copy()
        for identity in list(data['components']):
            component = data['components'][identity]
            if component.get('exports', {}).get('model'):
                del data['components'][identity]
        data['components']['tool-preflight-fixture'] = {
            'command': 'python3', 'args': ['-B', '/runtime/polyglot_tool_preflight.py', 'fixture'],
            'exports': {'model': {'preflight_fixture': {}}}}
        alias = data['model']
        data['models'][alias].update(provider='preflight_fixture', model='tool-preflight')
        data['module_config']['model'] = {'preflight_fixture': {
            'descriptor': descriptor(), 'features': [], 'commands': tool_commands}}
        driver.write_json(runtime/'configs/eval.json', data)
        extra_mounts, environment = grader.agent_runtime(runtime, language)
        driver.toolchain_environment_override(runtime, environment)
        mounts += extra_mounts + [(driver.CORE/'crates/proteus-core/tests/fixtures/process_model.py',
                                  '/core/crates/proteus-core/tests/fixtures/process_model.py'),
                                 (driver.CORE/'examples/modules/component_runtime.py',
                                  '/core/examples/modules/component_runtime.py'),
                                 (Path(__file__).resolve(), '/runtime/polyglot_tool_preflight.py')]
        command = driver.sandbox(inner, mounts, workspace, runtime, environment)
        command.insert(1, '--unshare-net')
        status = runner.execute(command, output, timeout, {'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8'})
        journals = list((runtime/'sessions').rglob('journal.jsonl'))
        records = [json.loads(line) for path in journals for line in path.read_text().splitlines()]
        results = [row['payload']['value']['result'] for row in records if row['kind'] == 'tool_result_recorded']
        calls = [row['payload']['value'] for row in records if row['kind'] == 'tool_call_recorded'
                 and row['payload']['value']['phase'].get('phase') == 'requested']
        settled = [row['payload']['value'] for row in records if row['kind'] == 'turn_settled']
        okay = (status['returncode'] == 0 and not status['timed_out'] and len(results) == 2
                and all(result['ok'] and f'PREFLIGHT_OK_{index+1}' in result['output']
                        for index, result in enumerate(results))
                and len(settled) == 1 and settled[0]['status'] == 'success'
                and settled[0]['error'] is None
                and settled[0]['output']['text'] == 'TOOL_PREFLIGHT_COMPLETE')
        receipt = {'language': language, 'passed': okay, **status, 'paid_model_requests': 0,
                   'network': 'outer bubblewrap --unshare-net; staged credentials removed before execution',
                   'slots': original_slots, 'model_override': 'deterministic model/v13 fixture only',
                   'tool_calls': len(calls), 'tool_results': len(results), 'settlements': settled,
                   'cache_survives_separate_exec_commands': okay, 'tmpfs_isolated_per_tool': okay,
                   'tool_result_evidence': results}
    finally:
        runner.remove_auth(runtime)
    driver.write_json(output/'result.json', receipt)
    print(json.dumps({key: receipt[key] for key in ('language', 'passed', 'tool_calls', 'tool_results', 'wall_seconds')}), flush=True)
    return receipt


def main():
    if sys.argv[1:] == ['fixture']:
        fixture_main()
        return
    import polyglot_corpus as corpus
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=corpus.DATA/'tool-preflight')
    parser.add_argument('--languages', nargs='+', choices=LANGUAGES, default=list(LANGUAGES))
    parser.add_argument('--timeout', type=int, default=180)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    results = [run_language(args.output.resolve(), language, args.timeout) for language in args.languages]
    receipt = {'passed': all(row['passed'] for row in results), 'paid_model_requests': 0, 'languages': results}
    (args.output/'result.json').write_text(json.dumps(receipt, indent=2)+'\n')
    raise SystemExit(0 if receipt['passed'] else 1)


if __name__ == '__main__':
    main()

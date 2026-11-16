"""Offline authoritative grading and toolchain mounts for all Polyglot languages."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time
import re
import xml.etree.ElementTree as ET

from polyglot_corpus import CORE, DATA, SOURCE, digest, manifest, write_json

TOOLS = DATA/'toolchains'
PYTHON = CORE/'target/performance/coding-harness/grader-venv'
RUST = Path.home()/'.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu'


def runtime_mounts(language=None):
    """Read-only dependency/toolchain trees; no host home or credential state."""
    python_runtime = (PYTHON/'bin/python').resolve().parents[2]
    by_language = {
        'python': [(PYTHON, '/polyglot/python'), (python_runtime, str(python_runtime))],
        'rust': [(RUST, '/polyglot/rust'), (TOOLS/'cargo-home', '/polyglot/cargo-home')],
        'java': [((TOOLS/'jdk').resolve(), '/polyglot/jdk'),
                 (TOOLS/'gradle-8.7', '/polyglot/gradle'),
                 (TOOLS/'gradle-home/caches/modules-2', '/polyglot/gradle-deps/modules-2')],
        'javascript': [(TOOLS/'npm', '/polyglot/npm')], 'go': [],
        'cpp': [(TOOLS/'boost', '/polyglot/boost')],
    }
    return by_language[language] if language else [mount for mounts in by_language.values() for mount in mounts]


def runtime_env():
    return {'PATH': '/runtime/bin:/polyglot/gradle/bin:/polyglot/rust/bin:/polyglot/jdk/bin:/polyglot/python/bin:/usr/bin:/bin',
            'JAVA_HOME': '/polyglot/jdk', 'GRADLE_USER_HOME': '/tmp/polyglot-gradle',
            'GRADLE_RO_DEP_CACHE': '/polyglot/gradle-deps',
            'CARGO_HOME': '/tmp/polyglot-cargo', 'CARGO_NET_OFFLINE': 'true',
            'BOOST_ROOT': '/polyglot/boost/usr',
            'GOTOOLCHAIN': 'local', 'GOCACHE': '/tmp/polyglot-go', 'GOPROXY': 'off',
            'PYTHONDONTWRITEBYTECODE': '1', 'HOME': '/tmp', 'CI': 'true'}


def agent_runtime(runtime, language):
    """Private writable caches alongside read-only shared dependency inputs."""
    runtime = Path(runtime)
    environment = runtime_env()
    environment.pop('HOME')  # Preserve the isolated native model credential HOME.
    environment['CARGO_HOME'] = '/state/home/.cargo'
    environment['GRADLE_USER_HOME'] = '/state/home/.gradle'
    if language == 'rust':
        cargo = runtime/'home/.cargo'
        cargo.mkdir(parents=True, exist_ok=True)
        (cargo/'registry').symlink_to('/polyglot/cargo-home/registry')
    return runtime_mounts(language), environment


COMMANDS = {
    'python': ['/polyglot/python/bin/python', '-m', 'pytest', '-q', '-o', 'python_files=*_test.py'],
    'go': ['go', 'test', '-json', './...'],
    'rust': ['bash', '-c', 'mkdir -p "$CARGO_HOME"; ln -s /polyglot/cargo-home/registry "$CARGO_HOME/registry"; cargo test --offline -- --include-ignored'],
    'javascript': ['bash', '-c', 'ln -s /polyglot/npm/node_modules node_modules; npm run test -- --runInBand'],
    'java': ['/polyglot/gradle/bin/gradle', '--offline', '--no-daemon', '--console=plain', 'test'],
    'cpp': ['bash', '-c', 'cmake -S . -B build -DEXERCISM_RUN_ALL_TESTS=1 -DCMAKE_POLICY_VERSION_MINIMUM=3.5 -DBoost_NO_BOOST_CMAKE=ON -G "Unix Makefiles" && cmake --build build --parallel 1'],
}


def executed_tests(language, text, workspace):
    """Require observed test execution, not merely a successful process exit."""
    if language == 'python':
        values = re.findall(r'(\d+) passed', text)
    elif language == 'rust':
        values = re.findall(r'test result: (?:ok|FAILED)\. (\d+) passed;', text)
    elif language == 'cpp':
        values = re.findall(r'All tests passed \(\d+ assertions? in (\d+) test cases?\)', text)
    elif language == 'javascript':
        values = re.findall(r'Tests:\s+(?:\d+ skipped,\s+)?(\d+) passed', text)
    elif language == 'go':
        total = 0
        for line in text.splitlines():
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(row, dict) and row.get('Action') == 'pass' and row.get('Test'):
                total += 1
        return total
    elif language == 'java':
        total = 0
        for path in (workspace/'build/test-results/test').glob('*.xml'):
            node = ET.parse(path).getroot()
            total += int(node.get('tests', 0)) - int(node.get('skipped', 0))
        return total
    else:
        raise ValueError('Unsupported Polyglot language: ' + language)
    return sum(map(int, values))


def execute(workspace, language, timeout=180):
    target = '/app/' + workspace.name
    command = ['bwrap', '--die-with-parent', '--new-session', '--unshare-all',
               '--ro-bind', '/usr', '/usr', '--ro-bind', '/etc', '/etc',
               '--symlink', 'usr/bin', '/bin', '--symlink', 'usr/lib', '/lib',
               '--symlink', 'usr/lib64', '/lib64', '--proc', '/proc', '--dev', '/dev',
               '--tmpfs', '/tmp', '--dir', '/app', '--bind', str(workspace), target,
               '--chdir', target, '--clearenv']
    for source, target in runtime_mounts(language):
        if not source.exists():
            raise ValueError('Missing prepared toolchain: ' + str(source))
        command += ['--ro-bind', str(source), target]
    for key, value in runtime_env().items():
        command += ['--setenv', key, value]
    command += ['--', *COMMANDS[language]]
    started = time.monotonic()
    # File output bounds memory; the retained diagnostic is capped equally
    # for both arms. This is not a model response/token limit.
    with tempfile.TemporaryFile(dir=DATA) as stream:
        process = subprocess.Popen(command, stdout=stream, stderr=subprocess.STDOUT,
                                   start_new_session=True, stdin=subprocess.DEVNULL)
        expired = False
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            expired = True
            os.killpg(process.pid, signal.SIGKILL)
            code = process.wait()
        stream.seek(0)
        text = stream.read(128_000).decode(errors='replace')
    count = executed_tests(language, text, workspace)
    return {'returncode': code, 'timed_out': expired, 'output': text, 'executed_tests': count,
            'wall_seconds': time.monotonic()-started}


def safe_solution(workspace, name):
    path = workspace/name
    return (path.is_file() and not path.is_symlink()
            and path.resolve().is_relative_to(workspace)
            and all(not p.is_symlink() for p in path.parents if p != workspace.parent))


def grade(case, workspace, output=None, reference=False, timeout=180):
    case, workspace = Path(case).resolve(), Path(workspace).resolve()
    info = json.loads((case/'case.json').read_text())
    private = case/'grader'
    if manifest(private) != info['grader_hashes']:
        raise ValueError('Authoritative grader or reference changed')
    unchanged = all(safe_solution(workspace, name) and digest(workspace/name) == info['repo_hashes'][name]
                    for name in info['immutable_files'])
    missing = [name for name in info['solution_files'] if not safe_solution(workspace, name)]
    # Each attempt starts from the authoritative originals. Unknown candidate
    # files, build scripts, and test files can never replace grading inputs.
    with tempfile.TemporaryDirectory(prefix='grade-', dir=DATA) as temporary:
        work = Path(temporary)/info['exercise']
        shutil.copytree(private/'pristine', work)
        for name in info['solution_files']:
            if name in missing:
                continue
            target = work/name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(workspace/name, target)
        example_manifest = None
        if reference:
            for name in info['reference_files']:
                target = work/name
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(private/'reference'/name, target)
            if info['language'] == 'rust' and '.meta/Cargo-example.toml' in info['source_hashes']:
                example = SOURCE/'rust/exercises/practice'/info['exercise']/'.meta/Cargo-example.toml'
                if digest(example) != info['source_hashes']['.meta/Cargo-example.toml']:
                    raise ValueError('Upstream example dependency manifest changed')
                if digest(example) != info['repo_hashes']['Cargo.toml']:
                    shutil.copy2(example, work/'Cargo.toml')
                    example_manifest = {'source': '.meta/Cargo-example.toml', 'sha256': digest(example),
                                        'same_contract': False,
                                        'candidate_manifest_sha256': info['repo_hashes']['Cargo.toml'],
                                        'scope': 'reference control only; candidate Cargo.toml remains immutable'}
        result = execute(work, info['language'], timeout)
    result.update(case=info['id'], language=info['language'], fixture_unchanged=unchanged,
                  missing_solution_files=missing, reference_control=reference,
                  reference_manifest_override=example_manifest,
                  passed=result['returncode'] == 0 and result['executed_tests'] > 0
                  and unchanged and (reference or not missing),
                  isolation='bubblewrap; network disabled; only declared solution files overlaid')
    if output:
        write_json(Path(output), result)
    return result

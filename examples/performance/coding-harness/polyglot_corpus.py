#!/usr/bin/env python3
"""Prepare all pinned Polyglot tasks without exposing tests or reference code."""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor, as_completed
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import time

HERE = Path(__file__).resolve().parent
CORE = HERE.parents[2]
DATA = CORE / 'target/performance/polyglot-225'
SOURCE = CORE / 'target/performance/coding-harness/polyglot'
PIN = '7e0611e77b54e2dea774cdc0aa00cf9f7ed6144f'
COUNTS = {'cpp': 26, 'go': 39, 'java': 47, 'javascript': 49, 'python': 34, 'rust': 30}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def manifest(path):
    return {str(p.relative_to(path)): digest(p) for p in sorted(path.rglob('*'))
            if p.is_file() and '.git' not in p.relative_to(path).parts}


def write_json(path, data):
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + '\n')


def source_cases(source=SOURCE):
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=source, text=True).strip()
    if revision != PIN or subprocess.check_output(['git', 'status', '--porcelain'], cwd=source, text=True):
        raise ValueError('Polyglot source must be clean at ' + PIN)
    cases = [(lang, p) for lang in COUNTS
             for p in sorted((source/lang/'exercises/practice').iterdir()) if p.is_dir()]
    if dict(Counter(lang for lang, _ in cases)) != COUNTS:
        raise ValueError('The full 225-task language distribution changed')
    return cases


def activated_test(language, text):
    if language == 'javascript':
        return re.sub(r'\bxit\(', 'it(', re.sub(r'\bxtest\(', 'test(', text))
    if language == 'java':
        return re.sub(r'@Disabled\([^)]*\)\s*\n', '', text)
    return text


def reference_targets(language, files):
    solutions = [p for p in files['solution'] if p != 'Cargo.toml']
    result = {}
    for example in files.get('example', []):
        if language == 'java':
            target = 'src/main/java/' + Path(example).name
        elif language == 'cpp':
            choices = [p for p in solutions if Path(p).suffix == Path(example).suffix]
            if len(choices) != 1:
                raise ValueError(f'Ambiguous C++ example mapping: {example}')
            target = choices[0]
        else:
            if len(solutions) != 1:
                raise ValueError(f'Ambiguous example mapping: {language} {solutions}')
            target = solutions[0]
        result[example] = target
    return result


def prepare(output=DATA/'cases', source=SOURCE):
    cases = source_cases(source)
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    index = []
    for language, original in cases:
        name = language + '--' + original.name
        case = output/name
        repo, private = case/'repo', case/'grader'
        repo.mkdir(parents=True)
        private.mkdir()
        files = json.loads((original/'.meta/config.json').read_text())['files']
        solutions = [p for p in files['solution'] if p != 'Cargo.toml']
        pristine = private/'pristine'
        # Only the grader receives test sources. Neither the grader workspace
        # nor task workspace receives examples/approaches from upstream.
        shutil.copytree(original, pristine, ignore=shutil.ignore_patterns('.meta', '.approaches', '.docs'))
        transforms = []
        for test in files['test']:
            path = pristine/test
            before = digest(path)
            text = activated_test(language, path.read_text())
            path.write_text(text)
            if digest(path) != before:
                transforms.append({'path': test, 'source_sha256': before,
                                   'activated_sha256': digest(path)})
        for path in sorted(pristine.rglob('*')):
            if not path.is_file():
                continue
            relative = str(path.relative_to(pristine))
            # C++ Catch helper sources and Rust/Java tests must stay private,
            # including helpers not listed individually in metadata.
            parts = path.relative_to(pristine).parts
            configs = {'CMakeLists.txt', 'Cargo.toml', 'go.mod', 'go.sum', 'build.gradle', 'settings.gradle',
                       'gradlew', 'gradlew.bat', 'package.json', 'babel.config.js',
                       '.npmrc', '.eslintrc', 'LICENSE'}
            if relative not in solutions and relative not in configs and parts[0] != 'gradle':
                continue
            target = repo/relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)
        references = reference_targets(language, files)
        for original_name, target_name in references.items():
            target = private/'reference'/target_name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(original/original_name, target)
        docs = [original/'.docs'/name for name in ('introduction.md', 'instructions.md', 'instructions.append.md')]
        instructions = '\n\n'.join(p.read_text() for p in docs if p.exists())
        (repo/'instructions.md').write_text(instructions + '\n')
        prompt = ('Implement the exercise below in the existing workspace.\n\n' + instructions
                  + '\n\nAccepted implementation files:\n'
                  + '\n'.join('- ' + name for name in solutions)
                  + '\n\nUse only the existing declared dependencies and standard libraries. '
                    'The official tests are held outside this workspace and run after your final answer. '
                    'You may create local smoke tests, but only the accepted implementation files '
                    'are submitted to the official grader. Preserve the public API.\n')
        (case/'prompt.txt').write_text(prompt)
        repo_hashes = manifest(repo)
        info = {'format': 1, 'id': name, 'family': 'aider-polyglot-225', 'language': language,
                'exercise': original.name, 'source_rev': PIN,
                'source_url': f'https://github.com/Aider-AI/polyglot-benchmark/tree/{PIN}/{language}/exercises/practice/{original.name}',
                'source_hashes': manifest(original), 'repo_hashes': repo_hashes,
                'grader_hashes': manifest(private), 'solution_files': solutions,
                'immutable_files': [p for p in repo_hashes if p not in solutions],
                'test_files': files['test'], 'reference_files': list(references.values()),
                'prompt_sha256': digest(case/'prompt.txt'), 'test_activation': transforms,
                'grader_timeout_seconds': 180,
                'adaptations': ['Tools-enabled harness; test/reference files hidden from agent',
                                'Only declared solution files submitted; Cargo.toml immutable',
                                'JS xtest and xit activated; explicit optional test.skip retained; Java Disabled activation; Rust include-ignored; C++ all-tests',
                                'Host toolchains with pinned offline dependency caches; grader network disabled']}
        write_json(case/'case.json', info)
        index.append({'id': name, 'language': language, 'case_sha256': digest(case/'case.json')})
    write_json(output/'manifest.json', {'format': 1, 'source_rev': PIN, 'counts': COUNTS, 'cases': index})
    return output/'manifest.json'


def grade(case, workspace, output=None, reference=False, timeout=180):
    from polyglot_grader import grade as run_grade
    return run_grade(case, workspace, output, reference, timeout)


def control_disposition(name, result):
    """Predeclared upstream defects; never infer exclusions from model outcomes."""
    no_tests = name == 'go--counter'
    baseline_refactor = name in ('go--ledger', 'go--markdown', 'javascript--ledger',
                                'java--ledger', 'java--tree-building')
    refactor = baseline_refactor or name in ('go--tree-building', 'java--tree-building', 'python--tree-building')
    return {'evaluable': not no_tests,
            'reason': 'upstream_no_tests' if no_tests else None,
            'starter_pass_allowed': baseline_refactor,
            'task_kind': 'ungradable_test_authoring' if no_tests else 'refactor' if refactor else 'implementation',
            'reference_same_contract': not bool(result['reference'].get('reference_manifest_override'))}


def control_valid(result):
    return (not result['evaluable'] and result['reason'] == 'upstream_no_tests') or (
        result['reference']['passed'] and (not result['starter']['passed'] or result['starter_pass_allowed']))


def write_control(target, result):
    temporary = target.with_suffix('.json.pending-'+str(time.time_ns()))
    write_json(temporary, result)
    temporary.replace(target)


def controls(cases=DATA/'cases', output=DATA/'controls', languages=None, workers=2, retry_failed=False):
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    paths = [p for p in sorted(Path(cases).iterdir()) if (p/'case.json').is_file()
             and (not languages or json.loads((p/'case.json').read_text())['language'] in languages)]

    def check(path):
        target = output/(path.name+'.json')
        fingerprint = digest(path/'case.json')
        if target.exists():
            previous = json.loads(target.read_text())
            if previous['case_sha256'] != fingerprint:
                raise ValueError('Existing control refers to a changed case: ' + path.name)
            previous.update(control_disposition(path.name, previous))
            write_control(target, previous)
            if not retry_failed or control_valid(previous):
                return path.name, previous
            history = output/'preflight-history'
            history.mkdir(exist_ok=True)
            target.replace(history/(path.name + '-' + str(time.time_ns()) + '.json'))
        result = {'case_sha256': fingerprint,
                  'grader_sha256': digest(HERE/'polyglot_grader.py'),
                  'starter': grade(path, path/'repo'),
                  'reference': grade(path, path/'repo', reference=True)}
        result.update(control_disposition(path.name, result))
        write_control(target, result)
        return path.name, result

    valid = True
    with ThreadPoolExecutor(max_workers=workers) as pool:
        jobs = [pool.submit(check, path) for path in paths]
        for job in as_completed(jobs):
            name, result = job.result()
            passed = control_valid(result)
            valid = valid and passed
            print(name, 'CONTROL_OK' if passed else 'CONTROL_FAILED', flush=True)
    return valid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    prep = sub.add_parser('prepare')
    prep.add_argument('--output', type=Path, default=DATA/'cases')
    check = sub.add_parser('grade')
    check.add_argument('case', type=Path)
    check.add_argument('--workspace', type=Path, required=True)
    check.add_argument('--output', type=Path)
    check.add_argument('--reference', action='store_true')
    control = sub.add_parser('controls')
    control.add_argument('--cases', type=Path, default=DATA/'cases')
    control.add_argument('--output', type=Path, default=DATA/'controls')
    control.add_argument('--languages', nargs='+', choices=COUNTS)
    control.add_argument('--workers', type=int, choices=range(1, 5), default=2)
    control.add_argument('--retry-failed', action='store_true')
    args = parser.parse_args()
    if args.action == 'prepare':
        print(prepare(args.output))
        return 0
    if args.action == 'controls':
        return 0 if controls(args.cases, args.output, args.languages, args.workers, args.retry_failed) else 1
    result = grade(args.case, args.workspace, args.output, args.reference)
    print(json.dumps(result, indent=2))
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())

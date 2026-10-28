#!/usr/bin/env python3
"""Measure cached Cargo builds and one repeatable source edit; never clean target."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import statistics
import subprocess
import time
import tomllib


def command_output(command, project):
    return subprocess.check_output(command, cwd=project, text=True).strip()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def build(command, project, output, name):
    started = time.monotonic()
    with (output / f'{name}.stdout.jsonl').open('wb') as stdout, \
            (output / f'{name}.stderr.log').open('wb') as stderr:
        process = subprocess.Popen(command, cwd=project, stdout=stdout, stderr=stderr,
                                   start_new_session=True)
        try:
            _, status, usage = os.wait4(process.pid, 0)
            process.returncode = os.waitstatus_to_exitcode(status)
        except BaseException:
            # Finish the Cargo process group before restoring its source file.
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            raise
    elapsed = time.monotonic() - started
    artifacts = []
    for line in (output / f'{name}.stdout.jsonl').read_text().splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get('reason') == 'compiler-artifact':
            artifacts.append(dict(package_id=event['package_id'],
                                  target=event['target']['name'], fresh=event['fresh']))
    result = dict(phase=name, exit_code=process.returncode,
                  wall_seconds=elapsed,
                  user_seconds=usage.ru_utime, system_seconds=usage.ru_stime,
                  wait4_maxrss_kib=usage.ru_maxrss, artifacts=artifacts,
                  fresh_artifacts=sum(row['fresh'] for row in artifacts),
                  rebuilt_artifacts=sum(not row['fresh'] for row in artifacts),
                  fresh_packages=sorted({row['package_id'] for row in artifacts if row['fresh']}),
                  rebuilt_packages=sorted({row['package_id'] for row in artifacts if not row['fresh']}))
    (output / f'{name}.json').write_text(json.dumps(result, indent=2) + '\n')
    print(f"{name}: {result['wall_seconds']:.3f}s, exit {process.returncode}", flush=True)
    if process.returncode:
        raise RuntimeError(f'{name} failed; see {output / (name + ".stderr.log")}')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project', type=Path, required=True)
    parser.add_argument('--package', default='proteus-native')
    parser.add_argument('--profile', choices=['fast', 'release'], default='fast')
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--edit-file', type=Path, required=True)
    parser.add_argument('--old', required=True)
    parser.add_argument('--new', required=True)
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--timings', action='store_true')
    args = parser.parse_args()
    project, output = args.project.resolve(), args.output.resolve()
    source = (project / args.edit_file).resolve()
    relative = source.relative_to(project)
    if args.jobs < 1 or args.repeats < 1 or not args.old or args.old == args.new:
        parser.error('jobs/repeats must be positive; old must be nonempty and differ from new')
    if output.exists():
        parser.error(f'output already exists: {output}')
    command_output(['git', 'ls-files', '--error-unmatch', '--', str(relative)], project)
    if subprocess.run(['git', 'diff', '--quiet', 'HEAD', '--', str(relative)],
                      cwd=project).returncode:
        parser.error('edit-file must be clean relative to HEAD, including the index')
    original = source.read_bytes()
    old, new = args.old.encode(), args.new.encode()
    if original.count(old) != 1:
        parser.error(f'old must occur exactly once, found {original.count(old)}')
    edited = original.replace(old, new, 1)
    command = ['cargo', 'build', '--locked', '--profile', args.profile,
               '-p', args.package, '--jobs', str(args.jobs),
               '--message-format=json-render-diagnostics']
    if args.timings:
        command.append('--timings')
    manifest = tomllib.loads((project / 'Cargo.toml').read_text())
    report = dict(command=command, project=str(project), source=str(relative),
                  original_sha256=sha(original), edited_sha256=sha(edited),
                  commit=command_output(['git', 'rev-parse', 'HEAD'], project),
                  git_status=command_output(['git', 'status', '--porcelain'], project),
                  rustc=command_output(['rustc', '-Vv'], project),
                  cargo=command_output(['cargo', '-V'], project),
                  environment={key: value for key, value in os.environ.items()
                               if key.startswith(('CARGO_', 'RUST', 'SCCACHE_'))
                               and not any(secret in key for secret in ('TOKEN', 'KEY', 'SECRET', 'PASSWORD'))},
                  profiles=manifest.get('profile', {}), repeats=args.repeats,
                  cargo_lock_sha256=sha((project / 'Cargo.lock').read_bytes()),
                  maxrss_scope='Linux wait4 rusage of Cargo and its waited children; '
                               'not a simultaneous sum of process RSS or PSS',
                  phases=[], status='running')
    output.mkdir(parents=True)
    (output / 'source.original').write_bytes(original)
    def save():
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    save()
    try:
        report['phases'].append(build(command, project, output, 'warmup'))
        for repeat in range(1, args.repeats + 1):
            if source.read_bytes() != original:
                raise RuntimeError('source changed externally before repeat; refusing to overwrite')
            report['phases'].append(build(command, project, output, f'{repeat}-noop'))
            if source.read_bytes() != original:
                raise RuntimeError('source changed externally during noop; refusing to overwrite')
            source.write_bytes(edited)
            try:
                report['phases'].append(build(command, project, output, f'{repeat}-edit'))
            finally:
                if source.read_bytes() != edited:
                    raise RuntimeError('edited source changed externally; refusing to overwrite')
                source.write_bytes(original)
                report['source_restored_sha256'] = sha(source.read_bytes())
            report['phases'].append(build(command, project, output, f'{repeat}-restore'))
            save()
        report['summary'] = {}
        for phase in ('noop', 'edit', 'restore'):
            rows = [row for row in report['phases'] if row['phase'].endswith('-' + phase)]
            report['summary'][phase] = {
                key: dict(median=statistics.median(values), minimum=min(values), maximum=max(values))
                for key in ('wall_seconds', 'user_seconds', 'system_seconds', 'wait4_maxrss_kib')
                if (values := [row[key] for row in rows])}
        report['status'] = 'complete'
    except BaseException as error:
        report['status'], report['error'] = 'failed', repr(error)
        raise
    finally:
        report['final_source_sha256'] = sha(source.read_bytes())
        save()


if __name__ == '__main__':
    def terminate(*_):
        raise KeyboardInterrupt('received SIGTERM')
    signal.signal(signal.SIGTERM, terminate)
    main()

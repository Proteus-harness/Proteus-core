#!/usr/bin/env python3
"""Fresh-process journal recovery on disposable, locally generated snapshots."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import statistics
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'multi-agent'))
from proc import Tree


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--seeds', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--create-seeds', action='store_true')
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.seeds = args.seeds.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)  # Never reuse evidence from an earlier attempt.
    env = dict(os.environ, MALLOC_ARENA_MAX='2', LC_ALL='C')
    for key in ['MALLOC_TRIM_THRESHOLD_', 'GLIBC_TUNABLES']:
        env.pop(key, None)
    for key, directory in [('HOME', 'home'), ('XDG_CONFIG_HOME', 'xdg'), ('TMPDIR', 'tmp')]:
        path = args.output / directory
        path.mkdir()
        env[key] = str(path)
    cases = [('inline', 512, 128), ('blob', 128, 512)]
    rows = []
    for case, count, kib in cases:
        seed = args.seeds / case
        if args.create_seeds:
            subprocess.run([str(args.binary), 'seed', str(seed), str(count), str(kib)],
                           env=env, check=True, capture_output=True, timeout=120)
        fixture = json.loads((seed / 'fixture.json').read_text())
        journal = seed / fixture['session'] / 'journal.jsonl'
        original = digest(journal)
        for mode in ['recover', 'projection']:
            for repeat in range(3):
                work = args.output / f'{case}-{mode}-{repeat}'
                shutil.copytree(seed, work)
                usage = work / 'time.txt'
                command = ['/usr/bin/time', '-v', '-o', str(usage), str(args.binary), mode, str(work)]
                process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE,
                                           stderr=subprocess.PIPE, start_new_session=True)
                tree = Tree(process.pid)
                samples = []
                started = time.monotonic()
                try:
                    while process.poll() is None:
                        samples.append(tree.sample())
                        if time.monotonic() - started > 120:
                            raise TimeoutError('owned probe exceeded its deadline')
                        time.sleep(.05)
                    stdout, stderr = process.communicate(timeout=5)
                    assert process.returncode == 0, stderr.decode()[-2000:]
                finally:
                    if process.poll() is None:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.communicate(timeout=5)
                measurement = json.loads(stdout)
                peak_rss_kib = int(re.search(r'Maximum resident set size \(kbytes\): (\d+)', usage.read_text())[1])
                verification = subprocess.run([str(args.binary), 'verify', str(work)],
                                              env=env, check=True, capture_output=True, timeout=120)
                assert digest(journal) == original, 'seed journal changed'
                if mode == 'projection':
                    assert digest(work / fixture['session'] / 'journal.jsonl') == original
                resident = [sum(r['Pss'] + r.get('SwapPss', 0) for r in s['processes']
                                if r['name'] == args.binary.name[:15]) for s in samples]
                resident = [value for value in resident if value > 0]
                assert resident, 'no owned probe memory samples'
                row = dict(case=case, mode=mode, repeat=repeat, snapshots=count, text_kib=kib,
                           elapsed_ms=measurement['elapsed_ms'], peak_rss_mib=peak_rss_kib / 1024,
                           settled_pss_swap_mib=statistics.median(resident[-15:]) / 1024,
                           verification=json.loads(verification.stdout), seed_sha256=original)
                rows.append(row)
                (work / 'samples.json').write_text(json.dumps(samples))
                print(json.dumps(row), flush=True)
    report = dict(binary=str(args.binary), binary_sha256=digest(args.binary), runs=rows,
                  scope='Repeated identical history replacements, 64 MiB payload total per case; '
                        'fresh processes/copies, warm file cache, glibc arenas=2, no models/modules. '
                        'Recover retains writer, projection retains all records. '
                        'Peak RSS is kernel ru_maxrss; settled PSS is the median of the final '
                        '15 live-probe samples, excluding the time wrapper and exited processes. '
                        'Verification is a separate untimed process; no production data.')
    (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()

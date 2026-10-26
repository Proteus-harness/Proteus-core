#!/usr/bin/env python3
"""Measure fresh Linux processes with identical synthetic long histories."""
import argparse
import json
from pathlib import Path
import platform
import resource
import statistics
import subprocess
import time

ROOT = Path(__file__).resolve().parent


def pressure():
    return {line.split()[0]: int(line.split('total=')[1])
            for line in Path('/proc/pressure/memory').read_text().splitlines()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rows', type=int, nargs='+', default=[1000, 5000, 20000])
    parser.add_argument('--repeats', type=int, default=3)
    args = parser.parse_args()
    if args.repeats < 1 or any(n < 50 for n in args.rows):
        parser.error('repeats >= 1 and rows >= 50 required')
    run = ROOT / 'target' / ('memory-' + time.strftime('%Y%m%d-%H%M%S'))
    run.mkdir(parents=True)
    binary = ROOT / 'target/release/session-memory'
    results = []

    def invoke(engine, mode, directory, count):
        pressure_before = pressure()
        usage_before = resource.getrusage(resource.RUSAGE_CHILDREN)
        started = time.perf_counter()
        process = subprocess.run([str(binary), engine, mode, str(directory), str(count)],
                                 check=True, capture_output=True, text=True, timeout=180)
        row = json.loads(process.stdout)
        wall = time.perf_counter() - started
        usage_after = resource.getrusage(resource.RUSAGE_CHILDREN)
        pressure_after = pressure()
        row['process_wall_ms'] = wall * 1000
        row['process_cpu_ms'] = ((usage_after.ru_utime - usage_before.ru_utime)
                                 + (usage_after.ru_stime - usage_before.ru_stime)) * 1000
        row['major_page_faults'] = usage_after.ru_majflt - usage_before.ru_majflt
        row['system_memory_stall_pct'] = {
            k: (pressure_after[k] - pressure_before[k]) / (wall * 1_000_000) * 100
            for k in pressure_before}
        results.append(row)
        print(json.dumps(row), flush=True)
        return row

    for count in args.rows:
        directory = run / str(count)
        directory.mkdir()
        for engine in ('jsonl', 'sqlite', 'turso'):
            invoke(engine, 'seed', directory, count)
        scenarios = [(e, m) for e in ('jsonl', 'sqlite', 'turso') for m in ('eager', 'paged')]
        samples = []
        for iteration in range(args.repeats):
            for engine, mode in scenarios if iteration % 2 == 0 else reversed(scenarios):
                samples.append(invoke(engine, mode, directory, count))
        for key in ('history_digest', 'tail_digest', 'visited_rows'):
            assert len({row[key] for row in samples}) == 1, (count, key, 'different results')
        for row in samples:
            assert row['retained_rows'] == (count if row['mode'] == 'eager' else 50)
        # Always leave a useful partial report if a later dataset/run fails.
        (run / 'runs.json').write_text(json.dumps(results, indent=2) + '\n')

    medians = []
    for count in args.rows:
        for engine, mode in scenarios:
            selected = [r for r in results if r['rows'] == count and r['engine'] == engine and r['mode'] == mode]
            median = {'rows': count, 'engine': engine, 'mode': mode}
            for key in ('ready_ms', 'scan_ms', 'process_wall_ms', 'process_cpu_ms', 'major_page_faults'):
                median[key] = statistics.median(r[key] for r in selected)
            median['system_memory_stall_pct'] = {
                k: statistics.median(r['system_memory_stall_pct'][k] for r in selected)
                for k in selected[0]['system_memory_stall_pct']}
            for stage in ('baseline', 'open', 'ready', 'scanned', 'released'):
                median[stage] = {k: statistics.median(r['memory'][stage][k] for r in selected)
                                 for k in selected[0]['memory'][stage]}
            medians.append(median)
    report = {
        'scope': 'controlled access-pattern prototype, not the current Proteus runtime or a migration',
        'data': 'synthetic AppTranscriptMessage-shaped Unicode/tool-result JSON, identical bytes in every engine',
        'conditions': 'fresh process per scenario, warm OS file cache, Linux self smaps_rollup, sequential runs',
        'cache': 'SQLite and Turso cache_size=-2048 (2 MiB), WAL, synchronous=FULL',
        'versions': {'turso': '0.8.0', 'rusqlite': '0.41.0', 'kernel': platform.release()},
        'semantics': {'eager': 'retain all decoded messages', 'paged': 'retain at most 50 decoded messages; JSONL scans to find the tail, SQL uses an indexed keyset query'},
        'limits': 'No model context, full app/GPU, journal validation/replay, crash recovery, concurrent writer, or real-session measurements. Instrumented wall times include validation and memory sampling; concurrent system load is not controlled. Linux VmHWM is approximate and excludes swap.',
        'equal_results': True, 'repeats': args.repeats, 'medians': medians, 'runs': results,
    }
    (run / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'report': str(run / 'report.json'), 'medians': medians}), flush=True)


if __name__ == '__main__':
    main()

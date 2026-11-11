"""Aggregate completed sample records from independent recorder streams."""
import json
import math
from pathlib import Path
import statistics


def _number(value):
    return type(value) in (int, float) and math.isfinite(value) and value >= 0


def _integer(value):
    return type(value) is int and value >= 0


def summarize(paths):
    paths = list(dict.fromkeys(Path(path).resolve() for path in paths))
    phases = {}
    ignored = 0
    for run, path in enumerate(paths):
        previous = None
        with path.open('rb') as stream:
            for raw in stream:
                if not raw.endswith(b'\n'):
                    ignored += 1
                    continue
                try:
                    row = json.loads(raw)
                except (ValueError, UnicodeError) as error:
                    raise ValueError('invalid JSONL event') from error
                if not isinstance(row, dict) or row.get('type') not in ('phase', 'sample', 'summary'):
                    raise ValueError('unknown event')
                if row['type'] != 'sample':
                    continue
                try:
                    phase = row['phase']
                    sequence, stamp = row['sequence'], row['monotonic_ns']
                    if not _integer(sequence) or not _integer(stamp) or not (phase is None or isinstance(phase, str) and phase):
                        raise ValueError('bad sample identity')
                    if previous and (sequence <= previous['sequence'] or stamp <= previous['monotonic_ns']):
                        raise ValueError('nonmonotonic samples')
                    if not all(_number(row[key]) for key in ('rss_kib', 'pss_swap_kib')):
                        raise ValueError('bad memory measurement')
                    if not isinstance(row['processes'], list) or not isinstance(row['memory_incomplete_pids'], list):
                        raise ValueError('bad process metadata')
                    identities = []
                    for process in row['processes']:
                        pid, start = process['pid'], process['start_ticks']
                        if not _integer(pid) or not pid or not _integer(start):
                            raise ValueError('bad process identity')
                        identities.append((run,pid,start))
                except (KeyError, TypeError) as error:
                    raise ValueError('bad sample fields') from error
                data = phases.setdefault(phase, dict(rss=[], pss=[], runs=set(), identities=set(), elapsed=0.0, cpu=0.0, incomplete=0))
                data['rss'].append(row['rss_kib'])
                data['pss'].append(row['pss_swap_kib'])
                data['runs'].add(run)
                data['identities'].update(identities)
                data['incomplete'] += bool(row['memory_incomplete_pids'])
                duration, cpu = row.get('interval_seconds'), row.get('interval_cpu_percent_one_core')
                if (previous and previous['phase'] == phase and sequence == previous['sequence']+1
                        and _number(duration) and duration > 0 and _number(cpu)):
                    data['elapsed'] += duration
                    data['cpu'] += duration * cpu / 100
                previous = row
    result = []
    for phase in sorted(phases, key=lambda p:(p is not None,p or '')):
        d=phases[phase]
        result.append(dict(phase=phase,samples=len(d['rss']),runs=len(d['runs']),median_rss_kib=statistics.median(d['rss']),
            median_pss_swap_kib=statistics.median(d['pss']),peak_rss_kib=max(d['rss']),peak_pss_swap_kib=max(d['pss']),
            cpu_seconds=d['cpu'],cpu_percent_one_core=100*d['cpu']/d['elapsed'] if d['elapsed'] else None,
            observed_processes=len(d['identities']),incomplete_memory_samples=d['incomplete']))
    return dict(runs=len(paths),ignored_partial_lines=ignored,phases=result)

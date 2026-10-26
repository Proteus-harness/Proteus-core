#!/usr/bin/env python3
"""Compare fresh processes on local disk; no OS cache drop or private sessions."""
import json
from pathlib import Path
import statistics
import subprocess
import time

ROOT = Path(__file__).resolve().parent
run = ROOT / 'target' / ('measure-' + time.strftime('%Y%m%d-%H%M%S'))
run.mkdir(parents=True)
binary = ROOT / 'target/release/proteus-session-db-bench'
results = []

def invoke(engine, mode):
    started = time.perf_counter()
    process = subprocess.run([str(binary), engine, mode, str(run / (engine + '.db'))],
                             check=True, capture_output=True, text=True, timeout=180)
    row = json.loads(process.stdout)
    row['process_wall_ms'] = (time.perf_counter() - started) * 1000
    results.append(row)
    print(json.dumps(row), flush=True)
    return row

for engine in ('sqlite', 'turso'):
    invoke(engine, 'seed')
for iteration in range(7):
    for engine in (('sqlite', 'turso') if iteration % 2 == 0 else ('turso', 'sqlite')):
        invoke(engine, 'read')
        invoke(engine, 'write')
for key in ('list_digest', 'page_digest'):
    assert len({row[key] for row in results if key in row}) == 1, 'different query results'
medians = {}
for engine in ('sqlite', 'turso'):
    medians[engine] = {}
    for key in ('open_ms', 'first_list_ms', 'first_page_ms', 'warm_page_mean_ms', 'durable_100_events_ms'):
        medians[engine][key] = statistics.median(row[key] for row in results if row['engine'] == engine and key in row)
report = {'schema': '512 sessions, 102400 Unicode JSON messages, LIMIT 50 keyset pages',
          'durability': 'WAL, synchronous=FULL, 100 separate autocommit events per write run',
          'versions': {'turso': '0.8.0', 'rusqlite': '0.41.0'},
          'scope': 'fresh processes, warm OS file cache, local filesystem, single writer, synthetic data',
          'equal_query_results': True, 'medians': medians, 'runs': results}
(run / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps({'report': str(run / 'report.json'), 'medians': medians}), flush=True)

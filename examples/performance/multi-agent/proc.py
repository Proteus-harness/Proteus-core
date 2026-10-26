"""Linux process-tree observations, including descendants reparented during teardown."""
import os
from pathlib import Path

PROC = Path('/proc')


def table():
    result = {}
    for path in PROC.iterdir():
        if not path.name.isdigit():
            continue
        try:
            raw = (path / 'stat').read_text()
            end = raw.rindex(')')
            fields = raw[end + 2:].split()
            result[int(path.name)] = {
                'parent': int(fields[1]), 'name': raw[raw.index('(')+1:end],
                'start_ticks': int(fields[19]),
                'cpu_ticks': int(fields[11]) + int(fields[12]),
                'minor_faults': int(fields[7]), 'major_faults': int(fields[9]),
            }
        except (OSError, ValueError):
            continue
    return result


class Tree:
    def __init__(self, root):
        self.root = root
        self.owned = {}
        self.last = {}

    def sample(self):
        current = table()
        roots = {pid for pid, start in self.owned.items()
                 if pid in current and current[pid]['start_ticks'] == start}
        if self.root in current:
            roots.add(self.root)
        while True:
            more = {pid for pid, row in current.items() if row['parent'] in roots} - roots
            if not more:
                break
            roots.update(more)
        rows = []
        for pid in sorted(roots):
            row = dict(current[pid], pid=pid)
            self.owned[pid] = row['start_ticks']
            path = PROC / str(pid)
            try:
                for line in (path / 'smaps_rollup').read_text().splitlines():
                    key, _, value = line.partition(':')
                    if key in ('Rss', 'Pss', 'SwapPss', 'Private_Clean', 'Private_Dirty'):
                        row[key] = int(value.split()[0])
                for line in (path / 'io').read_text().splitlines():
                    key, _, value = line.partition(':')
                    row[key] = int(value)
                for line in (path / 'status').read_text().splitlines():
                    key, _, value = line.partition(':')
                    if key in ('VmHWM', 'Threads'):
                        row[key] = int(value.split()[0])
            except (OSError, ValueError):
                continue
            self.last[(pid, row['start_ticks'])] = row
            rows.append(row)
        return dict(processes=rows,
                    pss_swap_kib=sum(p.get('Pss', 0)+p.get('SwapPss', 0) for p in rows),
                    rss_kib=sum(p.get('Rss', 0) for p in rows))

    def totals(self):
        fields = ('cpu_ticks', 'wchar', 'write_bytes', 'syscw', 'rchar', 'read_bytes',
                  'syscr', 'minor_faults', 'major_faults')
        totals = {field: sum(row.get(field, 0) for row in self.last.values()) for field in fields}
        totals['cpu_seconds'] = totals['cpu_ticks'] / os.sysconf('SC_CLK_TCK')
        totals['observed_processes'] = len(self.last)
        return totals

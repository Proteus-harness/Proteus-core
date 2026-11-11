#!/bin/sh
set -eu
cp /grader/reference/phase_report.py examples/performance/common/phase_report.py
cp /grader/reference/summarize.py examples/performance/summarize.py
cat >> examples/performance/common/README.md <<'DOC'

## Offline sample phases

`phase_report.summarize(paths)` streams recorder JSONL files into per-phase memory medians/peaks, unique process identities and weighted multicore CPU. Use `python examples/performance/summarize.py --samples-before before.jsonl --samples-after after.jsonl --output comparison.md`; each sample flag is repeatable. Gapped, cross-phase and cross-run intervals are excluded from CPU. A final incomplete line is reported and ignored.
DOC

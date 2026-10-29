#!/usr/bin/env python3
"""Observe dispatch while reusing the protocol's real multiplexed worker."""

from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "proteus-module-protocol/tests/fixtures"))
import multiplex_worker


class ObservedWorker(multiplex_worker.Worker):
    def receive(self, frame):
        marker = self._input(frame).get("started_path")
        if marker:
            Path(marker).write_text(frame["id"])
        super().receive(frame)


multiplex_worker.Worker = ObservedWorker
raise SystemExit(multiplex_worker.main())

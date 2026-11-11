"""File helpers. New snapshot publication needs crash-safe replacement."""
import json


def read_snapshot(path):
    if not path.exists():
        return {"version": 2, "last_seq": 0, "values": {}, "seen": {}}
    return json.loads(path.read_text())


def atomic_json(path, value):
    path.write_text(json.dumps(value))

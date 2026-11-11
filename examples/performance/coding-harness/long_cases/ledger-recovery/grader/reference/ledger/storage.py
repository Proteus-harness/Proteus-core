import json
import os
import tempfile
from .codec import encoded, validate_event, json_value
from .errors import CorruptJournal, LedgerError


def empty():
    return {"version": 2, "last_seq": 0, "values": {}, "seen": {}}


def apply(values, event):
    if event["op"] == "put":
        values[event["key"]] = event["value"]
    else:
        values.pop(event["key"], None)


def read_snapshot(path):
    if not path.exists():
        return empty()
    try:
        obj = json.loads(path.read_bytes())
        if type(obj) is not dict or set(obj) != {"version", "last_seq", "values", "seen"}:
            raise ValueError("snapshot fields")
        if type(obj["version"]) is not int or obj["version"] != 2 or type(obj["last_seq"]) is not int or obj["last_seq"] < 0:
            raise ValueError("snapshot version or sequence")
        if type(obj["seen"]) is not dict or type(obj["values"]) is not dict:
            raise ValueError("snapshot values and seen")
        seen_seq = set()
        replay = {}
        for key, entry in obj["seen"].items():
            if type(entry) is not dict or set(entry) != {"seq", "event"}:
                raise ValueError("snapshot seen entry")
            validate_event(entry["event"])
            seq = entry["seq"]
            if type(seq) is not int or not 0 < seq <= obj["last_seq"] or seq in seen_seq or key != entry["event"]["id"]:
                raise ValueError("snapshot event index")
            seen_seq.add(seq)
        for entry in sorted(obj["seen"].values(), key=lambda e: e["seq"]):
            apply(replay, entry["event"])
        json_value(obj["values"])
        if encoded(replay) != encoded(obj["values"]) or (not obj["seen"] and obj["last_seq"] != 0):
            raise ValueError("snapshot state mismatch")
        return obj
    except (ValueError, TypeError, KeyError, UnicodeError, RecursionError) as exc:
        raise CorruptJournal(str(exc)) from exc


def sync_directory(directory):
    fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic_json(path, value):
    fd, name = tempfile.mkstemp(prefix=".snapshot-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(encoded(value) + b"\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, path)
        sync_directory(path.parent)
    finally:
        if os.path.exists(name):
            os.unlink(name)

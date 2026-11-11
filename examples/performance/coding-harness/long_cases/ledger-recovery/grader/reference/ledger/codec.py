import hashlib
import json
import math
from .errors import LedgerError, CorruptJournal


def json_value(value, active=None):
    if value is None or type(value) in (str, bool, int):
        return
    if type(value) is float and math.isfinite(value):
        return
    if type(value) not in (list, dict) or (type(value) is dict and not all(type(k) is str for k in value)):
        raise LedgerError("invalid JSON value")
    active = set() if active is None else active
    if id(value) in active:
        raise LedgerError("cyclic JSON value")
    active.add(id(value))
    try:
        for item in value.values() if type(value) is dict else value:
            json_value(item, active)
    finally:
        active.remove(id(value))


def validate_event(event):
    if type(event) is not dict or event.get("op") not in ("put", "delete"):
        raise LedgerError("invalid event")
    keys = {"id", "op", "key", "value"} if event["op"] == "put" else {"id", "op", "key"}
    if set(event) != keys or any(type(event[k]) is not str or not event[k] for k in ("id", "key")):
        raise LedgerError("invalid event fields")
    if event["op"] == "put":
        json_value(event["value"])
    return event


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False).encode("utf-8")


def encode_frame(seq, event):
    if type(seq) is not int or seq <= 0:
        raise LedgerError("invalid sequence")
    validate_event(event)
    body = {"seq": seq, "event": event}
    return encoded({**body, "sha256": hashlib.sha256(encoded(body)).hexdigest()}) + b"\n"


def decode_frame(line):
    try:
        obj = json.loads(line)
        if type(obj) is not dict or set(obj) != {"seq", "event", "sha256"}:
            raise ValueError("invalid frame fields")
        expected = json.loads(encode_frame(obj["seq"], obj["event"]))["sha256"]
        if type(obj["sha256"]) is not str or obj["sha256"] != expected:
            raise ValueError("checksum mismatch")
        return obj["seq"], obj["event"]
    except (ValueError, TypeError, UnicodeError, KeyError, RecursionError) as exc:
        raise CorruptJournal(str(exc)) from exc

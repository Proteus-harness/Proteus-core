import json
import os
from pathlib import Path
from .codec import validate_event, encoded
from .errors import CorruptJournal, DuplicateEvent, LedgerError
from .store import Store
from .storage import empty, apply, atomic_json


def migrate(source, destination):
    data = Path(source).read_bytes()
    if data and not data.endswith(b"\n"):
        raise CorruptJournal("unfinished legacy record")
    target = empty()
    for line in data.splitlines():
        try:
            event = validate_event(json.loads(line))
        except (ValueError, TypeError, KeyError, UnicodeError) as exc:
            raise CorruptJournal(str(exc)) from exc
        known = target["seen"].get(event["id"])
        if known:
            if encoded(known["event"]) != encoded(event):
                raise CorruptJournal("legacy event id conflict")
            continue
        target["last_seq"] += 1
        target["seen"][event["id"]] = {"seq": target["last_seq"], "event": event}
        apply(target["values"], event)
    root = Path(destination)
    existing = Store.open(root)
    with existing._lock():
        existing._load()
        if (root / "snapshot.json").exists() or (root / "journal.log").exists():
            actual = {"version": 2, "last_seq": existing.last_seq, "values": existing._values, "seen": existing._seen}
            if encoded(actual) != encoded(target):
                raise LedgerError("destination differs from migration")
            return existing.last_seq
        atomic_json(root / "snapshot.json", target)
        with (root / "journal.log").open("wb") as stream:
            stream.flush()
            os.fsync(stream.fileno())
    return target["last_seq"]

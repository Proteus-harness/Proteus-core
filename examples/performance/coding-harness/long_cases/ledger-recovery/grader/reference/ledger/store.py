from contextlib import contextmanager
from copy import deepcopy
import fcntl
import os
from pathlib import Path
from .codec import decode_frame, encode_frame, validate_event, encoded
from .errors import CorruptJournal, DuplicateEvent
from .storage import read_snapshot, atomic_json, apply


class Store:
    def __init__(self, root, recover=True):
        self.root = Path(root)
        self.recover = recover
        self._load()

    @classmethod
    def open(cls, root, recover=True):
        return cls(root, recover)

    @contextmanager
    def _lock(self):
        self.root.mkdir(parents=True, exist_ok=True)
        with (self.root / ".lock").open("a+b") as lock:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(lock.fileno(), fcntl.LOCK_UN)

    def _load(self):
        snapshot = read_snapshot(self.root / "snapshot.json")
        self.last_seq = snapshot["last_seq"]
        covered = self.last_seq
        self._values, self._seen = snapshot["values"], snapshot["seen"]
        journal = self.root / "journal.log"
        data = journal.read_bytes() if journal.exists() else b""
        cut = data.rfind(b"\n") + 1
        if cut != len(data) and not self.recover:
            raise CorruptJournal("unfinished journal record")
        self._valid_bytes = cut
        previous = None
        for line in data[:cut].splitlines(keepends=True):
            seq, event = decode_frame(line)
            if previous is not None and seq != previous + 1:
                raise CorruptJournal("nonconsecutive journal")
            previous = seq
            known = self._seen.get(event["id"])
            if known and encoded(known["event"]) != encoded(event):
                raise CorruptJournal("conflicting event id")
            if seq <= covered:
                if not known or known["seq"] > seq:
                    raise CorruptJournal("covered prefix missing from snapshot")
                continue
            if seq != self.last_seq + 1:
                raise CorruptJournal("journal sequence gap")
            if not known:
                apply(self._values, event)
                self._seen[event["id"]] = {"seq": seq, "event": event}
            self.last_seq = seq

    def _append(self, event):
        validate_event(event)
        event = deepcopy(event)
        with self._lock():
            self._load()
            known = self._seen.get(event["id"])
            if known:
                if encoded(known["event"]) != encoded(event):
                    raise DuplicateEvent("event id already committed with other content")
                return known["seq"]
            seq = self.last_seq + 1
            frame = encode_frame(seq, event)
            with (self.root / "journal.log").open("a+b") as stream:
                stream.truncate(self._valid_bytes)
                stream.write(frame)
                stream.flush()
                os.fsync(stream.fileno())
            apply(self._values, event)
            self._seen[event["id"]] = {"seq": seq, "event": event}
            self.last_seq = seq
            return seq

    def put(self, key, value, *, event_id):
        return self._append({"id": event_id, "op": "put", "key": key, "value": value})

    def delete(self, key, *, event_id):
        return self._append({"id": event_id, "op": "delete", "key": key})

    def get(self, key, default=None):
        return deepcopy(self._values.get(key, default))

    def items(self):
        return deepcopy(self._values)

    def compact(self):
        with self._lock():
            self._load()
            snapshot = {"version": 2, "last_seq": self.last_seq, "values": self._values, "seen": self._seen}
            atomic_json(self.root / "snapshot.json", snapshot)
            with (self.root / "journal.log").open("wb") as stream:
                stream.flush()
                os.fsync(stream.fileno())
            self._valid_bytes = 0

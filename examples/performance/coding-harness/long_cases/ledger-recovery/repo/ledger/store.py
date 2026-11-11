import json
from pathlib import Path
from .codec import decode_frame, encode_frame
from .storage import read_snapshot, atomic_json


class Store:
    def __init__(self, root, recover=True):
        self.root = Path(root)
        self.recover = recover
        self._load()

    @classmethod
    def open(cls, root, recover=True):
        return cls(root, recover)

    def _load(self):
        snap = read_snapshot(self.root / "snapshot.json")
        self.last_seq = snap["last_seq"]
        self._values = snap["values"]
        self._seen = snap["seen"]
        journal = self.root / "journal.log"
        if journal.exists():
            for line in journal.read_bytes().splitlines():
                seq, event = decode_frame(line)
                self.last_seq = seq
                self._apply(event, seq)

    def _apply(self, event, seq):
        if event["op"] == "put":
            self._values[event["key"]] = event["value"]
        else:
            self._values.pop(event["key"], None)
        self._seen[event["id"]] = {"seq": seq, "event": event}

    def _append(self, event):
        self.root.mkdir(parents=True, exist_ok=True)
        seq = self.last_seq + 1
        with (self.root / "journal.log").open("ab") as stream:
            stream.write(encode_frame(seq, event))
        self._apply(event, seq)
        self.last_seq = seq
        return seq

    def put(self, key, value, *, event_id):
        return self._append({"id": event_id, "op": "put", "key": key, "value": value})

    def delete(self, key, *, event_id):
        return self._append({"id": event_id, "op": "delete", "key": key})

    def get(self, key, default=None):
        return self._values.get(key, default)

    def items(self):
        return self._values

    def compact(self):
        atomic_json(self.root / "snapshot.json", {"version": 2, "last_seq": self.last_seq,
                     "values": self._values, "seen": self._seen})
        (self.root / "journal.log").write_bytes(b"")

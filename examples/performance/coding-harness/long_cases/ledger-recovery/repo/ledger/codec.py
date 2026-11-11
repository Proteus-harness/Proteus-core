"""The old implementation did not checksum frames or validate JSON values."""
import json
from .errors import LedgerError, CorruptJournal


def validate_event(event):
    if not isinstance(event, dict) or "id" not in event:
        raise LedgerError("missing event id")
    return event


def encode_frame(seq, event):
    validate_event(event)
    return (json.dumps({"seq": seq, "event": event}) + "\n").encode()


def decode_frame(line):
    try:
        obj = json.loads(line)
        return obj["seq"], validate_event(obj["event"])
    except Exception as exc:
        raise CorruptJournal(str(exc)) from exc

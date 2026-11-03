import uuid

identity = str(uuid.uuid4())
print(json.dumps({"type": "event", "event": {"type": "runtime", "envelope": {
    "schema_version": 2, "event_id": str(uuid.uuid4()),
    "session_id": identity, "thread_id": identity, "turn_id": None,
    "seq": 1, "timestamp_ms": 0, "event": {"SessionStarted": {
        "session_id": identity, "cwd": str(pathlib.Path.cwd()), "model": None,
        "session_dir": str(pathlib.Path.cwd() / identity)}}}}}), flush=True)

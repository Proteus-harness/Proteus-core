"""Post-measurement canonical journal verification, one record/blob at a time."""
import hashlib
import json
from pathlib import Path


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def records(session):
    session = Path(session).resolve()
    previous = None
    with (session / "journal.jsonl").open() as stream:
        for line in stream:
            record = json.loads(line)
            if record["schema_version"] != 21:
                raise ValueError("expected canonical journal schema 21")
            sequence = record["session_seq"]
            if previous is not None and sequence != previous + 1:
                raise ValueError("journal sequence gap")
            previous = sequence
            stored = record["payload"]
            if stored["storage"] == "inline":
                payload = stored["value"]
            elif stored["storage"] == "blob":
                sha = stored["sha256"]
                relative = Path(stored["relative_path"])
                if len(sha) != 64 or relative != Path("blobs") / f"{sha}.json":
                    raise ValueError("invalid canonical blob reference")
                path = session / relative
                if not path.resolve().is_relative_to(session) or path.stat().st_size != stored["bytes"] or digest(path) != sha:
                    raise ValueError("canonical blob size/hash mismatch")
                with path.open() as blob:
                    payload = json.load(blob)
            else:
                raise ValueError("unsupported canonical payload storage")
            yield record, payload


def text(message):
    return "\n".join(part["payload"]["Text"]["text"] for part in message["parts"]
                     if "Text" in part["payload"])


def message_digest(messages):
    value = hashlib.sha256()
    count = 0
    for message in messages:
        # IDs, part IDs, roles, metadata and text must all survive unchanged.
        encoded = json.dumps(message, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
        value.update(len(encoded).to_bytes(8, "little"))
        value.update(encoded)
        count += 1
    return count, value.hexdigest()


def seed_summary(session, expected_rows, text_repeats, model_history_rows=None):
    if model_history_rows is not None and not 1 <= model_history_rows <= expected_rows:
        raise ValueError("model history must retain 1..ROWS messages")
    seed = None
    mutations = 0
    for record, payload in records(session):
        mutations += 1
        if record["kind"] != "history_mutated":
            raise ValueError("expected only typed-seeder history mutations")
        messages = payload["messages"]
        if seed is not None:
            report = payload.get("compaction") or {}
            if (mutations != 2 or model_history_rows is None or payload["mutation"] != "replace"
                    or report.get("changed") is not True or report.get("summary_source") != "history-scale-fixture-tail"
                    or report.get("input_messages") != expected_rows or report.get("output_messages") != model_history_rows
                    or message_digest(messages) != (model_history_rows, seed["model_messages_sha256"])):
                raise ValueError("expected exactly the declared synthetic tail compaction")
            seed["seed_compaction"] = report
            continue
        if payload["mutation"] != "append" or payload.get("compaction"):
            raise ValueError("expected one fresh typed-seeder history append")
        if len(messages) != expected_rows or len({message["id"] for message in messages}) != expected_rows:
            raise ValueError("seed message count/identity mismatch")
        for index, message in enumerate(messages):
            expected = f"Fixture row {index}\n" + "Синтетический текст для замера памяти и прокрутки. " * text_repeats
            if text(message) != expected or message["role"] != ("User" if index % 2 == 0 else "Assistant"):
                raise ValueError(f"typed fixture text/role mismatch at row {index}")
        count, sha = message_digest(messages)
        active = messages[-model_history_rows:] if model_history_rows is not None else messages
        active_count, active_sha = message_digest(active)
        seed = dict(rows=count, display_rows=count, messages_sha256=sha, session_id=record["session_id"],
                    model_history_rows=active_count, model_messages_sha256=active_sha,
                    model_ids=[message["id"] for message in active], seed_compaction=None,
                    head=[dict(message_id=message["id"], text=text(message)) for message in messages[:50]],
                    tail=[dict(message_id=message["id"], text=text(message)) for message in messages[-50:]],
                    ids=[message["id"] for message in messages])
    if seed is None:
        raise ValueError("empty canonical seed journal")
    if mutations != (2 if model_history_rows is not None else 1):
        raise ValueError("declared synthetic compaction is missing")
    seed["history_mutations"] = mutations
    return seed


def verify_active(session, seed, rounds):
    ui_ids = set(seed["ids"])
    ids = set(seed.get("model_ids", seed["ids"]))
    excluded_ids = ui_ids - ids
    active_rows = seed.get("model_history_rows", seed["rows"])
    active_sha = seed.get("model_messages_sha256", seed["messages_sha256"])
    first_requests = {}
    settled = []
    final_seed_sha = None
    count = 0
    requests = 0
    initial_mutations = 0
    seeded_ui_verified = False
    declared_compaction_seen = False
    for record, payload in records(session):
        kind = record["kind"]
        if kind == "history_mutated":
            messages = payload["messages"]
            if initial_mutations == 0:
                if (payload["mutation"] != "append" or payload.get("compaction")
                        or message_digest(messages) != (seed["rows"], seed["messages_sha256"])):
                    raise ValueError("seed append changed")
                initial_mutations += 1
                seeded_ui_verified = True
                count = len(messages)
                if not seed.get("seed_compaction"):
                    final_seed_sha = active_sha
                continue
            if payload.get("compaction"):
                if (initial_mutations != 1 or requests or settled or declared_compaction_seen
                        or payload["mutation"] != "replace" or payload["compaction"] != seed.get("seed_compaction")
                        or message_digest(messages) != (active_rows, active_sha)):
                    raise ValueError("fixture unexpectedly compacted history")
                declared_compaction_seen = True
                initial_mutations += 1
                count = len(messages)
                final_seed_sha = active_sha
                continue
            seen_count, sha = message_digest(message for message in messages if message["id"] in ids)
            if payload["mutation"] in ("replace", "checkpoint"):
                # A plain Replace would clear the durable UI transcript.
                if payload["mutation"] == "replace":
                    raise ValueError("uncompacted replacement would reset persistent UI history")
                if seen_count != active_rows or sha != active_sha:
                    raise ValueError("history checkpoint/replacement discarded or changed seeded rows")
                if any(message["id"] in excluded_ids for message in messages):
                    raise ValueError("checkpoint reintroduced compacted-away model messages")
                final_seed_sha = sha
                count = len(messages)
            elif payload["mutation"] == "append":
                count += len(messages)
                if seen_count:
                    if seen_count != active_rows or sha != active_sha:
                        raise ValueError("seed append changed")
                    final_seed_sha = sha
        elif kind == "model_request_recorded":
            requests += 1
            turn = record["turn_id"]
            messages = payload["request"]["messages"]
            seen_count, sha = message_digest(message for message in messages if message["id"] in ids)
            if (seen_count != active_rows or sha != active_sha
                    or any(message["id"] in excluded_ids for message in messages)):
                raise ValueError("model request lost or changed seeded canonical messages")
            first_requests.setdefault(turn, dict(total_messages=len(messages), seeded_messages=seen_count))
        elif kind == "turn_settled":
            if payload["status"] != "success":
                raise ValueError(f"peer turn failed: {payload}")
            settled.append(record["turn_id"])
    if len(settled) != rounds or len(first_requests) != rounds or requests != rounds * 2:
        raise ValueError(f"turn/model request count mismatch: {len(settled)}, {len(first_requests)}, {requests}")
    if (not seeded_ui_verified or declared_compaction_seen != bool(seed.get("seed_compaction"))
            or count <= active_rows or final_seed_sha != active_sha):
        raise ValueError("active history did not preserve seeded corpus")
    return dict(settled_turn_ids=settled, model_requests=requests, final_messages=count,
                seeded_messages=seed["rows"], seeded_messages_sha256=seed["messages_sha256"],
                display_rows=seed["rows"], model_history_rows=active_rows,
                active_seed_messages_sha256=final_seed_sha,
                first_requests=first_requests)

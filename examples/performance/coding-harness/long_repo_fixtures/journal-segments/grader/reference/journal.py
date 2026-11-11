"""Validated schema-21 journals, monolithic or manifest-segmented."""
import hashlib
import json
from pathlib import Path
import re


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def _file(session, relative):
    value = Path(relative)
    if value.is_absolute() or '..' in value.parts or not value.parts:
        raise ValueError('unsafe journal path')
    result = session
    for part in value.parts:
        result = result / part
        if result.is_symlink():
            raise ValueError('symlink journal path')
    if not result.is_file() or not result.resolve().is_relative_to(session):
        raise ValueError('missing or unsafe journal file')
    return result


def _sha(value):
    return isinstance(value, str) and re.fullmatch('[0-9a-f]{64}', value) is not None


def _decode(session, raw):
    if not raw.endswith(b'\n') or not raw.strip():
        raise ValueError('truncated or blank journal record')
    record = json.loads(raw)
    if not isinstance(record, dict) or record.get('schema_version') != 21 or type(record.get('schema_version')) is not int:
        raise ValueError('expected schema21')
    if type(record.get('session_seq')) is not int:
        raise ValueError('invalid sequence')
    stored = record['payload']
    if stored['storage'] == 'inline':
        payload = stored['value']
    elif stored['storage'] == 'blob':
        sha = stored['sha256']
        if not _sha(sha) or stored['relative_path'] != 'blobs/' + sha + '.json' or type(stored['bytes']) is not int or stored['bytes'] < 0:
            raise ValueError('invalid blob metadata')
        path = _file(session, stored['relative_path'])
        if path.stat().st_size != stored['bytes'] or digest(path) != sha:
            raise ValueError('blob hash or size mismatch')
        with path.open('rb') as blob:
            payload = json.load(blob)
    else:
        raise ValueError('unknown storage')
    return record, payload


def _source_paths(session):
    legacy = session / 'journal.jsonl'
    manifest = session / 'journal-manifest.json'
    if legacy.exists() == manifest.exists():
        raise ValueError('expected one journal format')
    if legacy.exists():
        yield _file(session, 'journal.jsonl'), None
        return
    with _file(session, 'journal-manifest.json').open('rb') as stream:
        spec = json.load(stream)
    if not isinstance(spec, dict) or set(spec) != {'version', 'segments'} or type(spec['version']) is not int or spec['version'] != 1 or not isinstance(spec['segments'], list):
        raise ValueError('invalid manifest')
    for index, entry in enumerate(spec['segments'], 1):
        if not isinstance(entry, dict) or set(entry) != {'relative_path', 'sha256', 'bytes', 'first_seq', 'last_seq'}:
            raise ValueError('invalid segment metadata')
        if entry['relative_path'] != f'segments/{index:06}.jsonl' or not _sha(entry['sha256']):
            raise ValueError('invalid segment identity')
        if any(type(entry[k]) is not int for k in ('bytes', 'first_seq', 'last_seq')) or entry['bytes'] < 0 or entry['first_seq'] > entry['last_seq']:
            raise ValueError('invalid segment ranges')
        path = _file(session, entry['relative_path'])
        if path.stat().st_size != entry['bytes'] or digest(path) != entry['sha256']:
            raise ValueError('segment hash or size mismatch')
        yield path, entry


def raw_records(session):
    session = Path(session).resolve()
    previous = None
    try:
        for path, entry in _source_paths(session):
            if entry:
                first = last = None
                with path.open('rb') as stream:
                    for raw in stream:
                        record, _ = _decode(session, raw)
                        seq = record['session_seq']
                        if last is not None and seq != last + 1:
                            raise ValueError('segment sequence gap')
                        if first is None:
                            first = seq
                        last = seq
                if first != entry['first_seq'] or last != entry['last_seq']:
                    raise ValueError('segment range mismatch')
                if previous is not None and first != previous + 1:
                    raise ValueError('cross-segment sequence gap')
            with path.open('rb') as stream:
                for raw in stream:
                    record, payload = _decode(session, raw)
                    sequence = record['session_seq']
                    if previous is not None and sequence != previous + 1:
                        raise ValueError('journal sequence gap')
                    previous = sequence
                    yield record, payload, raw
    except (OSError, KeyError, TypeError, IndexError, UnicodeError) as error:
        raise ValueError('invalid canonical journal') from error


def records(session):
    for record, payload, _ in raw_records(session):
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


def seed_summary(session, expected_rows, text_repeats, model_history_rows=None,
                 seed_model_requests=0, request_payload_kib=0):
    if model_history_rows is not None and not 1 <= model_history_rows <= expected_rows:
        raise ValueError("model history must retain 1..ROWS messages")
    seed = None
    mutations = 0
    requests, responses = set(), set()
    for record, payload in records(session):
        kind = record["kind"]
        if kind == "model_request_recorded" and seed_model_requests:
            if (seed is None or record.get("turn_id") is not None or payload.get("origin") != "auxiliary"
                    or payload.get("owner") != {"owner": "workflow"}):
                raise ValueError("unexpected synthetic auxiliary request attribution")
            exchange = payload["exchange_id"]
            messages = payload["request"]["messages"]
            if (exchange in requests or len(requests) >= seed_model_requests or not messages
                    or message_digest(messages[:-1]) != (seed["model_history_rows"], seed["model_messages_sha256"])):
                raise ValueError("synthetic request lost canonical history or duplicated an exchange")
            extra = messages[-1]
            content = text(extra)
            size = request_payload_kib * 1024
            prefix = "SYNTHETIC_REQUEST_PAYLOAD_"
            if (extra["role"] != "System" or len(extra["parts"]) != 1 or not content.isascii()
                    or len(content) != size or (size and (not content.startswith(prefix)
                                                         or content[len(prefix):].strip("x")))):
                raise ValueError("synthetic request payload differs from declared bytes")
            requests.add(exchange)
            continue
        if kind == "model_response_recorded" and seed_model_requests:
            exchange = payload["exchange_id"]
            if (exchange not in requests or exchange in responses or record.get("turn_id") is not None
                    or payload["outcome"].get("status") != "response"):
                raise ValueError("synthetic response lifecycle mismatch")
            responses.add(exchange)
            continue
        if kind != "history_mutated":
            raise ValueError("expected only typed-seeder history mutations")
        if requests:
            raise ValueError("synthetic exchanges must follow immutable history mutations")
        mutations += 1
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
    if len(requests) != seed_model_requests or responses != requests:
        raise ValueError("declared synthetic model exchanges are missing")
    seed["history_mutations"] = mutations
    seed["stored_model_requests"] = len(requests)
    seed["stored_request_extra_payload_bytes"] = len(requests) * request_payload_kib * 1024
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

"""Independent release-contract checks. No reference implementation is imported."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
from unittest.mock import patch
import pytest

sys.path.insert(0, "/app")
from ledger import Store, migrate, LedgerError, CorruptJournal, DuplicateEvent
from ledger.codec import encode_frame, decode_frame


def event(n, value=None, key=None, op="put"):
    obj = {"id": str(n), "op": op, "key": key or ("key-" + str(n))}
    if op == "put":
        obj["value"] = value
    return obj


def wire(seq, value):
    body = {"seq": seq, "event": value}
    digest = hashlib.sha256(json.dumps(body, ensure_ascii=False, sort_keys=True,
                                      separators=(",", ":"), allow_nan=False).encode()).hexdigest()
    return (json.dumps({**body, "sha256": digest}, ensure_ascii=False) + "\n").encode()


def disk(root):
    return {str(p.relative_to(root)): p.read_bytes() for p in root.rglob("*") if p.is_file() and p.name != ".lock"}


def cli(root, *args):
    return subprocess.run([sys.executable, "-m", "ledger", "--root", str(root), *args],
                          cwd="/app", capture_output=True, text=True, timeout=15)


def test_public_release_suite():
    result = subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-v"],
                            cwd="/app", capture_output=True, text=True, timeout=30)
    assert result.returncode == 0, result.stdout + result.stderr


def test_codec_independent_unicode_and_nested_json():
    obj = event("nested", {"a": [None, True, False, {"é": "雪"}], "n": -3.25})
    encoded = encode_frame(721, obj)
    assert encoded.endswith(b"\n") and encoded.count(b"\n") == 1
    assert json.loads(encoded) == json.loads(wire(721, obj))
    assert decode_frame(wire(721, obj)) == (721, obj)
    assert decode_frame(wire(721, obj)[:-1]) == (721, obj)


@pytest.mark.parametrize("invalid", [0, -1, True, 1.5, "1"])
def test_codec_rejects_invalid_sequence(invalid):
    with pytest.raises(LedgerError):
        encode_frame(invalid, event("a", 3))


@pytest.mark.parametrize("obj", [
    {"id": "a", "op": "delete", "key": "k", "value": None},
    {"id": "a", "op": "put", "key": "k"},
    {"id": "", "op": "put", "key": "k", "value": 0},
    {"id": "a", "op": "put", "key": "", "value": 0},
    {"id": "a", "op": "increment", "key": "k"},
    {"id": "a", "op": "put", "key": "k", "value": {"nested": [float("nan")]}},
    {"id": "a", "op": "put", "key": "k", "value": {7: "nonstring key"}},
])
def test_invalid_append_is_read_only(tmp_path, obj):
    store = Store.open(tmp_path)
    store.put("kept", 9, event_id="baseline")
    before = disk(tmp_path)
    with pytest.raises(LedgerError):
        if obj["op"] == "delete":
            encode_frame(2, obj)
        elif set(obj) != {"id", "op", "key", "value"} or obj["op"] != "put":
            encode_frame(2, obj)
        else:
            store.put(obj["key"], obj["value"], event_id=obj["id"])
    assert disk(tmp_path) == before


def test_new_open_is_read_only_and_copy_isolation(tmp_path):
    root = tmp_path / "uncreated"
    first = Store.open(root)
    assert first.last_seq == 0 and first.items() == {} and not root.exists()
    value = {"list": [1, {"x": "original"}]}
    first.put("item", value, event_id="created")
    value["list"][1]["x"] = "caller changed"
    read = first.get("item")
    read["list"].append(5)
    all_items = first.items()
    all_items["item"]["list"].clear()
    assert first.get("item") == {"list": [1, {"x": "original"}]}


def test_torn_tail_preserved_until_append(tmp_path):
    prefix = wire(1, event("1", "old", key="x"))
    journal = tmp_path / "journal.log"
    journal.write_bytes(prefix + b'\xff{"partial')
    assert Store.open(tmp_path).items() == {"x": "old"}
    assert journal.read_bytes() == prefix + b'\xff{"partial'
    assert Store.open(tmp_path).put("x", "new", event_id="2") == 2
    assert journal.read_bytes().startswith(prefix)
    assert Store.open(tmp_path, recover=False).items() == {"x": "new"}


@pytest.mark.parametrize("data", [b"\n", b"{}\n", b"not-json\n", b"\xff\n"])
def test_complete_bad_tail_is_corruption(tmp_path, data):
    journal = tmp_path / "journal.log"
    journal.write_bytes(wire(1, event("1", 4)) + data)
    before = journal.read_bytes()
    with pytest.raises(CorruptJournal):
        Store.open(tmp_path)
    assert journal.read_bytes() == before


def test_checksum_and_gap_are_corruption(tmp_path):
    journal = tmp_path / "journal.log"
    journal.write_bytes(wire(1, event("1", 4)).replace(b'"value": 4', b'"value": 5'))
    with pytest.raises(CorruptJournal):
        Store.open(tmp_path)
    journal.write_bytes(wire(1, event("1", 4)) + wire(3, event("3", 7)))
    with pytest.raises(CorruptJournal):
        Store.open(tmp_path)


def test_duplicate_disk_frame_consumes_sequence_without_reapplying(tmp_path):
    journal = tmp_path / "journal.log"
    one, two = event("a", "first", key="same"), event("b", "second", key="same")
    journal.write_bytes(wire(1, one) + wire(2, two) + wire(3, one))
    store = Store.open(tmp_path)
    assert store.last_seq == 3 and store.get("same") == "second"
    before = journal.read_bytes()
    assert store.put("same", "first", event_id="a") == 1
    assert journal.read_bytes() == before
    with pytest.raises(DuplicateEvent):
        store.delete("same", event_id="a")
    assert journal.read_bytes() == before


def test_snapshot_prefix_replay_after_interruption(tmp_path):
    journal = tmp_path / "journal.log"
    first, second = event("one", 11, key="x"), event("two", None, key="x", op="delete")
    journal.write_bytes(wire(1, first) + wire(2, second))
    store = Store.open(tmp_path)
    store.compact()
    snapshot = json.loads((tmp_path / "snapshot.json").read_text())
    assert snapshot["last_seq"] == 2 and snapshot["values"] == {}
    journal.write_bytes(wire(1, first) + wire(2, second) + wire(3, event("three", [4], key="x")))
    restarted = Store.open(tmp_path)
    assert restarted.last_seq == 3 and restarted.get("x") == [4]
    restarted.compact()
    assert Store.open(tmp_path).delete("x", event_id="two") == 2
    assert Store.open(tmp_path).get("x") == [4]


def test_failed_snapshot_replace_preserves_all_previous_bytes(tmp_path):
    store = Store.open(tmp_path)
    store.put("x", [1], event_id="1")
    store.compact()
    store.put("x", [2], event_id="2")
    (tmp_path / "foreign-file").write_text("preserved")
    before = disk(tmp_path)
    with patch("os.replace", side_effect=OSError("injected snapshot publication failure")):
        with pytest.raises(OSError):
            store.compact()
    assert disk(tmp_path) == before
    assert Store.open(tmp_path).get("x") == [2]


def test_append_and_snapshot_fsync(tmp_path):
    store = Store.open(tmp_path)
    real = os.fsync
    kinds = []
    import stat
    def sync(fd):
        kinds.append("directory" if stat.S_ISDIR(os.fstat(fd).st_mode) else "file")
        return real(fd)
    with patch("os.fsync", side_effect=sync):
        store.put("x", 1, event_id="1")
        assert "file" in kinds
        kinds.clear()
        store.compact()
    assert kinds.count("file") >= 2 and "directory" in kinds


@pytest.mark.parametrize("bad", [
    {"version": 1, "last_seq": 0, "values": {}, "seen": {}},
    {"version": 2, "last_seq": True, "values": {}, "seen": {}},
    {"version": 2, "last_seq": 4, "values": {}, "seen": {}},
    {"version": 2, "last_seq": 1, "values": {"wrong": 4}, "seen": {"1": {"seq": 1, "event": event("1", 3)}}},
])
def test_bad_snapshot_is_never_silently_reinitialized(tmp_path, bad):
    target = tmp_path / "snapshot.json"
    target.write_text(json.dumps(bad))
    before = disk(tmp_path)
    with pytest.raises(CorruptJournal):
        Store.open(tmp_path)
    assert disk(tmp_path) == before


def test_process_writers_lock_reload_and_deduplicate(tmp_path):
    code = "from ledger import Store; import sys; s=Store.open(sys.argv[1]); n=sys.argv[2]; [s.put('k'+n+'-'+str(i), i, event_id=n+'-'+str(i)) for i in range(18)]; s.put('common', 'same', event_id='same')"
    procs = [subprocess.Popen([sys.executable, "-c", code, str(tmp_path), str(n)], cwd="/app", stdout=subprocess.PIPE, stderr=subprocess.PIPE) for n in range(4)]
    for proc in procs:
        out, err = proc.communicate(timeout=25)
        assert proc.returncode == 0, err.decode()
    store = Store.open(tmp_path)
    assert store.last_seq == 73 and len(store.items()) == 73
    store.compact()
    assert Store.open(tmp_path).put("common", "same", event_id="same") <= 73
    assert Store.open(tmp_path).last_seq == 73


def test_migration_validates_before_effects_and_preserves_source(tmp_path):
    source, destination = tmp_path / "legacy", tmp_path / "target"
    source.write_bytes(json.dumps(event("a", 3)).encode() + b"\n{}\n")
    before = source.read_bytes()
    with pytest.raises(CorruptJournal):
        migrate(source, destination)
    assert not destination.exists() and source.read_bytes() == before
    source.write_bytes((json.dumps(event("a", 3)) + "\n").encode() * 2 + (json.dumps(event("b", 4)) + "\n").encode())
    before = source.read_bytes()
    assert migrate(source, destination) == 2
    assert migrate(source, destination) == 2
    assert source.read_bytes() == before
    assert Store.open(destination).items() == {"key-a": 3, "key-b": 4}
    kept = disk(destination)
    source.write_text(json.dumps(event("new", 9)) + "\n")
    with pytest.raises(LedgerError):
        migrate(source, destination)
    assert disk(destination) == kept


def test_cli_json_null_missing_and_conflict(tmp_path):
    ok = cli(tmp_path, "put", "unicode", '"кошка"', "--id", "one")
    assert ok.returncode == 0 and json.loads(ok.stdout) == {"seq": 1} and not ok.stderr
    null = cli(tmp_path, "put", "nil", "null", "--id", "two")
    assert null.returncode == 0
    read = cli(tmp_path, "get", "nil")
    assert read.returncode == 0 and json.loads(read.stdout) is None
    missing = cli(tmp_path, "get", "missing")
    assert missing.returncode == 4 and not missing.stdout and isinstance(json.loads(missing.stderr)["error"], str)
    conflict = cli(tmp_path, "put", "unicode", "0", "--id", "one")
    assert conflict.returncode == 2 and not conflict.stdout and "error" in json.loads(conflict.stderr)
    bad = cli(tmp_path, "put", "nonfinite", "NaN", "--id", "three")
    assert bad.returncode == 2 and not bad.stdout
    assert json.loads(cli(tmp_path, "dump").stdout) == {"nil": None, "unicode": "кошка"}


def test_boolean_and_number_ids_are_different_events(tmp_path):
    store = Store.open(tmp_path)
    store.put("x", True, event_id="same")
    before = disk(tmp_path)
    with pytest.raises(DuplicateEvent):
        store.put("x", 1, event_id="same")
    assert disk(tmp_path) == before
    store.compact()
    with pytest.raises(DuplicateEvent):
        Store.open(tmp_path).put("x", 1, event_id="same")


def test_nonjson_cycle_rejected_before_any_persistence(tmp_path):
    root = tmp_path / "new"
    value = []; value.append(value)
    with pytest.raises(LedgerError):
        Store.open(root).put("x", value, event_id="bad")
    assert not root.exists()

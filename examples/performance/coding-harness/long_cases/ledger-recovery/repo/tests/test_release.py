import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from ledger import Store, migrate, DuplicateEvent, CorruptJournal
from ledger.codec import encode_frame, decode_frame


class ReleaseTests(unittest.TestCase):
    def test_codec_wire_checksum(self):
        event = {"id": "a", "op": "put", "key": "emoji", "value": "🌊"}
        body = json.dumps({"seq": 1, "event": event}, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
        frame = json.loads(encode_frame(1, event))
        self.assertEqual(frame["sha256"], hashlib.sha256(body).hexdigest())
        self.assertEqual(decode_frame(encode_frame(1, event)), (1, event))

    def test_retry_and_conflict(self):
        with tempfile.TemporaryDirectory() as d:
            store = Store.open(d)
            self.assertEqual(store.put("a", [1], event_id="first"), 1)
            self.assertEqual(store.put("a", [1], event_id="first"), 1)
            with self.assertRaises(DuplicateEvent):
                store.put("a", [2], event_id="first")
            self.assertEqual(Store.open(d).last_seq, 1)

    def test_torn_tail(self):
        with tempfile.TemporaryDirectory() as d:
            store = Store.open(d)
            store.put("a", 3, event_id="first")
            with (Path(d) / "journal.log").open("ab") as f:
                f.write(b'{"seq":2')
            self.assertEqual(Store.open(d).get("a"), 3)
            with self.assertRaises(CorruptJournal):
                Store.open(d, recover=False)
            Store.open(d).delete("a", event_id="second")
            self.assertEqual(Store.open(d).items(), {})

    def test_compact_retry(self):
        with tempfile.TemporaryDirectory() as d:
            store = Store.open(d)
            store.put("a", None, event_id="one")
            store.compact()
            self.assertEqual(Store.open(d).put("a", None, event_id="one"), 1)
            self.assertEqual(Store.open(d).items(), {"a": None})

    def test_two_open_writers(self):
        with tempfile.TemporaryDirectory() as d:
            first, second = Store.open(d), Store.open(d)
            self.assertEqual(first.put("a", 1, event_id="one"), 1)
            self.assertEqual(second.put("b", 2, event_id="two"), 2)
            self.assertEqual(Store.open(d).items(), {"a": 1, "b": 2})

    def test_migration_fixture(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(migrate("fixtures/legacy.jsonl", d), 3)
            self.assertEqual(migrate("fixtures/legacy.jsonl", d), 3)
            self.assertEqual(Store.open(d).items(), {"colour": "синий"})


if __name__ == "__main__":
    unittest.main()

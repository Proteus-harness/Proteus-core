# Event and file contract

All data is UTF-8 JSON. Python bool is not a sequence number. Sequence numbers
are positive integers, or zero only for an empty snapshot's last_seq. Event ids
and keys are nonempty strings. An event is exactly one of these shapes.

- `{"id": ID, "op": "put", "key": KEY, "value": JSON_VALUE}`
- `{"id": ID, "op": "delete", "key": KEY}`

Event equality uses the canonical JSON encoding defined below, so true and 1
are different values. No unknown event fields are accepted. JSON_VALUE may include nested lists and
objects with string keys, null, booleans, strings, integers and finite floats.
Reject NaN/infinity and non-JSON Python objects with LedgerError before mutation.
Caller mutation after put/get/items must not mutate the store's state.

A frame is exactly `{"seq": N, "event": EVENT, "sha256": HEX}` plus one LF byte.
HEX is lowercase SHA256 of the UTF-8 encoding of the following JSON object,
using json.dumps(sort_keys=True, separators=(",", ":"), ensure_ascii=False,
allow_nan=False). It covers both sequence and event.

`{"seq": N, "event": EVENT}`

`ledger.codec.encode_frame(seq,event)` returns those complete bytes.
`decode_frame(line)` accepts a bytes line with or without its terminating LF and
returns `(seq,event)`. Invalid complete frames, invalid checksum, invalid event
or sequence raise CorruptJournal. Extra frame fields are corruption.

A snapshot is exactly `{"version":2,"last_seq":N,"values":OBJECT,"seen":OBJECT}`.
`seen` maps every retained id to `{"seq":FIRST_SEQUENCE,"event":EVENT}`.
These first sequences must be unique positive integers no greater than last_seq.
The id key must equal event.id. Replaying seen entries in first-sequence order
must reproduce values exactly. Empty seen requires last_seq=0 and empty values.
Snapshots preserve the complete event-id deduplication index across compaction.
Corrupt snapshots raise CorruptJournal, never reset the ledger to empty.

`Store.open(root,recover=True)` opens a ledger. A nonexistent root is an empty
ledger and opening it creates no files. `put(key,value,event_id=...)` and
`delete(key,event_id=...)` return the committed sequence. Repeating the exact
same event id and semantic event returns its first sequence and changes no
journal bytes. Reusing an id for different content raises DuplicateEvent and
changes no journal bytes. Delete of a missing key still commits an event.
`last_seq` includes all committed frames. `get(key,default=None)` and `items()`
return independent values. The constructor and open accept pathlib paths.

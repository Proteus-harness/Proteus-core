# Recovery, publication and migration

Journal records must be consecutive and increasing. After a snapshot, the first
uncovered frame must be snapshot.last_seq+1. A file can still contain an already
covered prefix from before snapshot publication. That prefix must itself be
consecutive and its events must agree with the snapshot's seen index. Ignore
its effects, then replay the uncovered suffix. This supports a crash after
snapshot replacement but before journal reset. A duplicate event with identical
content in an uncovered frame is a valid no-op, consumes that frame's sequence,
and retains its first-sequence index. Conflicting ids are corruption on replay.

With recover=True, ignore only a final non-LF-terminated byte fragment. A complete
newline-terminated malformed line is corruption even at EOF. Empty lines are
corruption. With recover=False any final fragment is corruption. Opening and
reading never change disk. Before appending, discard an ignored fragment and
append the new frame immediately after the valid prefix. Keep validated bytes
unchanged. Invalid append input, conflicting ids and replay errors cause no
journal or snapshot mutation.

Writers and compaction use a root-local `.lock` with fcntl.flock LOCK_EX, reload
under the lock, and serialize across independently opened Store objects and
processes. Flush journal writes with os.fsync before reporting success. A call
must work even if another writer committed since this Store was opened.

compact() reloads under that same lock. Publish snapshot through a temporary
file in the same directory, flush and fsync it, os.replace it over snapshot.json,
then fsync its directory. Only after publication may journal.log be truncated
and fsynced. A failure of snapshot replacement must leave the previous snapshot
and journal byte-identical and clean up its temporary file. A successful compact
of an empty/new root is supported. Existing foreign files are preserved.

Legacy migration reads a source JSONL file containing unwrapped events. Every
record must have LF termination. Blank, incomplete or invalid records fail with
CorruptJournal. Repeated identical ids are deduplicated in first-seen order,
conflicts fail. The entire input must be validated before creating destination
files. Unique legacy events receive sequences 1..N. The source is immutable.
`migrate(source,destination)` publishes a v2 snapshot and empty journal, returning
N. An existing v2 destination can be reused only when its complete reconstructed
values, seen index and last_seq match this migration. A rerun then returns N.
A divergent or corrupt destination is rejected without changing its files.
Do not overwrite unrelated files in either source or destination directories.

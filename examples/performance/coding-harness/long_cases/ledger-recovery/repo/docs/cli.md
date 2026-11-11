# CLI compatibility

The entrypoint is `python -m ledger --root DIR COMMAND ...`.

- `put KEY JSON --id ID` and `delete KEY --id ID` print `{"seq":N}` and exit 0.
- `get KEY` prints the stored JSON value, including null, and exits 0. Missing
  keys exit 4 and print a single JSON error object to stderr, with no stdout.
- `dump` prints the current key/value JSON object and exits 0.
- `compact` prints `{"seq":N}` and exits 0.
- `migrate SOURCE` prints `{"seq":N}` and exits 0.

Invalid data, conflicting event ids, corrupt persistence and relevant OSError
failures exit 2, print a single `{"error":STRING}` object to stderr and leave
stdout empty. ArgumentParser's normal usage errors may use its ordinary output.
Use sorted JSON object keys, ensure_ascii=False, allow_nan=False for output.

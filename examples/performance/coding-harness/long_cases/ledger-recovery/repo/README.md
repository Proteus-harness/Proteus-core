# Ledger 2 recovery release

This small stdlib-only persistence package has a working prototype and several
unimplemented durability requirements. Update its existing modules for the
release contract in `docs/format-v2.md`, `docs/recovery.md`, and `docs/cli.md`.
The public Python API and command grammar are already used by clients.

Run public checks with `python -m unittest discover -s tests -v`. Files in
`tests/`, `docs/`, and `fixtures/` are release inputs and must remain unchanged.
The independent evaluator also exercises fresh processes and interrupted states.

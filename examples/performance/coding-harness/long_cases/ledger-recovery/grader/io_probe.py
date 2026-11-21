"""Run one public OS-contract probe with instrumentation before solution import."""
import os
from pathlib import Path
import stat
import sys
from unittest.mock import patch


def disk(root):
    return {str(p.relative_to(root)): p.read_bytes()
            for p in root.rglob("*") if p.is_file() and p.name != ".lock"}


def failed_snapshot_replace(root):
    real = os.replace
    armed = False
    injected = False

    def replace(*args, **kwargs):
        nonlocal injected
        if armed:
            injected = True
            raise OSError("injected snapshot publication failure")
        return real(*args, **kwargs)

    # Direct imports and default-argument captures must see the same wrapper
    # as module-qualified os.replace. Setup still calls the real operation.
    with patch("os.replace", new=replace):
        from ledger import Store
        store = Store.open(root)
        store.put("x", [1], event_id="1")
        store.compact()
        store.put("x", [2], event_id="2")
        (root / "foreign-file").write_text("preserved")
        before = disk(root)
        armed = True
        try:
            store.compact()
        except OSError:
            pass
        else:
            raise AssertionError("snapshot replacement failure was not propagated")
        if not injected:
            raise AssertionError("snapshot replacement fault was not reached")
        armed = False
    if disk(root) != before:
        raise AssertionError("failed snapshot replacement changed existing bytes or left temporary files")
    if Store.open(root).get("x") != [2]:
        raise AssertionError("reopening after failed replacement lost the latest value")


def append_and_snapshot_fsync(root):
    real = os.fsync
    kinds = []

    def sync(fd):
        kinds.append("directory" if stat.S_ISDIR(os.fstat(fd).st_mode) else "file")
        return real(fd)

    with patch("os.fsync", new=sync):
        from ledger import Store
        store = Store.open(root)
        kinds.clear()  # Import/open activity is outside the append check.
        store.put("x", 1, event_id="1")
        if "file" not in kinds:
            raise AssertionError("append did not fsync a file")
        kinds.clear()
        store.compact()
    if kinds.count("file") < 2 or "directory" not in kinds:
        raise AssertionError(f"compact did not fsync two files and a directory: {kinds}")


if __name__ == "__main__":
    # The grader launches this fresh interpreter with its solution cwd (/app).
    # The probe file lives in /grader, so explicitly expose the solution root.
    sys.path.insert(0, str(Path.cwd()))
    probes = {"replace": failed_snapshot_replace, "fsync": append_and_snapshot_fsync}
    probes[sys.argv[1]](Path(sys.argv[2]))

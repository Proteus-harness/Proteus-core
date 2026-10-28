"""Untimed, fresh-runtime display checks for compacted active-history fixtures."""
import json
from pathlib import Path
import selectors
import subprocess
import time
from urllib.parse import urlencode
from urllib.request import urlopen

from active import abort_unobserved_group
from journal import digest


def display_rows(snapshot):
    return len(snapshot["transcript"]) + (snapshot.get("history") or {}).get("before", 0)


def check_page(page, expected, total):
    actual = [dict(message_id=message["message_id"], text=message["text"]) for message in page["messages"]]
    if page["total"] != total or actual != expected:
        raise ValueError("cold runtime display page lost seeded message IDs/text")


def probe(binary, config, workspace, session, seed, output, *, exact=False, timeout=60):
    """Read only small display pages; never substitute active history count."""
    session, output = Path(session), Path(output)
    journal = session / "journal.jsonl"
    original = digest(journal)
    command = [str(binary), "--config", str(config), "--cwd", str(workspace), "--resume-session", str(session),
               "server", "http", "--host", "127.0.0.1", "--port", "0", "--ready-stdout"]
    deadline = time.monotonic() + timeout
    with Path(str(output) + ".stderr.log").open("w") as error:
        process = subprocess.Popen(command, cwd=workspace, stdout=subprocess.PIPE, stderr=error,
                                   text=True, start_new_session=True)
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                origin = None
                while origin is None:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0 or not selector.select(remaining):
                        raise TimeoutError("cold UI probe HTTP readiness timed out")
                    line = process.stdout.readline()
                    if not line:
                        raise RuntimeError("cold UI probe exited before HTTP readiness")
                    if line.startswith("{"):
                        ready = json.loads(line)
                        if ready.get("type") == "http_ready":
                            origin = ready["origin"]
            query = {"session_dir": str(session)}
            with urlopen(origin + "/events?" + urlencode(query), timeout=min(30, max(1, deadline - time.monotonic()))) as response:
                snapshot = None
                for line in response:
                    if time.monotonic() >= deadline:
                        raise TimeoutError("cold UI snapshot readiness timed out")
                    if not line.startswith(b"data: "):
                        continue
                    record = json.loads(line[6:])
                    if record.get("type") == "response" and record.get("ok") is False:
                        raise ValueError(f"cold UI probe failed: {record}")
                    event = record.get("event", {})
                    if event.get("type") == "session_snapshot":
                        snapshot = event["snapshot"]
                        break
                if snapshot is None or snapshot["session_id"] != seed["session_id"]:
                    raise ValueError("cold UI probe resumed a different session")
                total = display_rows(snapshot)
                if total < seed["rows"] or (exact and total != seed["rows"]):
                    raise ValueError("cold runtime UI message count lost seeded display history")
                summary = dict(session_id=snapshot["session_id"], display_rows=total,
                               seeded_display_rows=seed["rows"], model_history_rows=seed.get("model_history_rows", seed["rows"]),
                               visible_snapshot_rows=len(snapshot["transcript"]))
            def page(**extra):
                with urlopen(origin + "/history/page?" + urlencode(dict(query, **extra)),
                             timeout=min(30, max(1, deadline - time.monotonic()))) as response:
                    return json.load(response)
            current = page()
            if current["total"] != total:
                raise ValueError("cold snapshot and display page totals differ")
            seed_tail = page(before=seed["rows"], revision=current["revision"])
            check_page(seed_tail, seed["tail"], total)
            saved = dict(summary=summary, seed_tail=seed_tail)
            if seed.get("head"):
                first = page(before=len(seed["head"]), revision=current["revision"])
                check_page(first, seed["head"], total)
                saved["seed_head"] = first
            output.write_text(json.dumps(saved, ensure_ascii=False, indent=2) + "\n")
        finally:
            # Do not reap/poll before group cleanup; its PID remains reserved.
            abort_unobserved_group(process)
    if digest(journal) != original:
        raise ValueError("read-only cold UI probe changed the canonical journal")
    return dict(summary, seeded_display_anchors_matched=True, journal_unchanged=True)

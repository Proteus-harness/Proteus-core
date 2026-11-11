"""Keep the public command grammar while adding structured errors."""
import argparse
import json
import sys
from .errors import LedgerError
from .store import Store
from .migrate import migrate


def run(argv=None):
    p = argparse.ArgumentParser()
    p.add_argument("--root", required=True)
    commands = p.add_subparsers(dest="command", required=True)
    for name in ["put", "delete"]:
        sub = commands.add_parser(name)
        sub.add_argument("key")
        if name == "put":
            sub.add_argument("value")
        sub.add_argument("--id", required=True)
    commands.add_parser("get").add_argument("key")
    commands.add_parser("dump")
    commands.add_parser("compact")
    commands.add_parser("migrate").add_argument("source")
    a = p.parse_args(argv)
    if a.command == "migrate":
        print(json.dumps({"seq": migrate(a.source, a.root)}))
        return 0
    store = Store.open(a.root)
    if a.command == "put":
        result = {"seq": store.put(a.key, json.loads(a.value), event_id=a.id)}
    elif a.command == "delete":
        result = {"seq": store.delete(a.key, event_id=a.id)}
    elif a.command == "get":
        if a.key not in store.items():
            print(json.dumps({"error": "missing key"}), file=sys.stderr)
            return 4
        result = store.get(a.key)
    elif a.command == "dump":
        result = store.items()
    else:
        store.compact()
        result = {"seq": store.last_seq}
    print(json.dumps(result, sort_keys=True, ensure_ascii=False, allow_nan=False))
    return 0


def main(argv=None):
    try:
        return run(argv)
    except (LedgerError, OSError, ValueError) as exc:
        print(json.dumps({"error": str(exc)}, sort_keys=True, ensure_ascii=False), file=sys.stderr)
        return 2

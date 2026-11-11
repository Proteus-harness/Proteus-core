import json
from pathlib import Path
from .store import Store


def migrate(source, destination):
    target = Store.open(destination)
    for line in Path(source).read_text().splitlines():
        event = json.loads(line)
        if event["op"] == "put":
            target.put(event["key"], event["value"], event_id=event["id"])
        else:
            target.delete(event["key"], event_id=event["id"])
    target.compact()
    return target.last_seq

"""Минимальная панель: исходники распространяются вместе с каталогом."""

import json
import sys

for line in sys.stdin:
    event = json.loads(line)
    if event["type"] == "render":
        print(json.dumps({
            "type": "view",
            "view": event["view"],
            "root": {"type": "text", "text": "Расширение установлено из каталога."},
        }, ensure_ascii=False), flush=True)

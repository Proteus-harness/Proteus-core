#!/usr/bin/env python3
"""Example extension for proteus-native: reads the agent server itself and
answers with widget trees. Protocol: one JSON object per line on stdio."""

import json
import sys
import urllib.request

# The agent server is local; system proxies must not intercept it.
DIRECT = urllib.request.build_opener(urllib.request.ProxyHandler({}))

server = {}
clicks = 0


def send(message):
    sys.stdout.write(json.dumps(message, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def api(path):
    request = urllib.request.Request(
        server["origin"] + path,
        headers={"Authorization": "Bearer " + server["token"]},
    )
    with DIRECT.open(request, timeout=10) as response:
        return json.load(response)


def panel():
    try:
        sessions = api("/sessions")
    except Exception as error:  # noqa: BLE001 - shown to the user
        return {"type": "text", "text": f"Сервер недоступен: {error}", "tone": "danger"}
    rows = [
        [s.get("preview") or "Новая сессия", str(s.get("message_count", 0))]
        for s in sessions[:20]
    ]
    return {
        "type": "stack",
        "gap": "medium",
        "children": [
            {
                "type": "stack",
                "direction": "row",
                "children": [
                    {"type": "metric", "label": "Сессий", "value": str(len(sessions))},
                    {"type": "metric", "label": "Нажатий", "value": str(clicks), "tone": "accent"},
                ],
            },
            {"type": "card", "title": "Последние", "children": [
                {"type": "table", "columns": ["Сессия", "Сообщений"], "rows": rows},
            ]},
            {"type": "stack", "direction": "row", "children": [
                {"type": "button", "label": "Нажать", "action": "click", "variant": "primary"},
                {"type": "button", "label": "Открыть документ", "action": "open"},
            ]},
        ],
    }


def document(argument):
    return {"type": "card", "title": f"Документ «{argument}»", "children": [
        {"type": "text", "text": "Документы открываются вкладками с аргументом."},
        {"type": "bar_chart", "bars": [
            {"label": "нажатия", "value": clicks},
            {"label": "пример", "value": 3, "tone": "success"},
        ]},
    ]}


def badge():
    return {"type": "badge", "label": str(clicks), "tone": "accent"}


views = {}
sequences = {}
view_sessions = {}


def render(view):
    surface, argument = views[view]
    if surface == "panel":
        root = panel()
    elif surface == "badge":
        root = badge()
    else:
        root = document(argument)
    send({"type": "view", "session_dir":view_sessions.get(view), "view": view, "root": root, "sequence": sequences.get(view, 0)})


for line in sys.stdin:
    message = json.loads(line)
    kind = message["type"]
    if kind == "hello":
        server = message["server"]
    elif kind == "render":
        view_sessions[message["view"]] = message.get("session_dir")
        views[message["view"]] = (message["surface"], message.get("argument"))
        render(message["view"])
    elif kind == "close":
        views.pop(message["view"], None)
    elif kind == "action":
        sequences[message["view"]] = message["sequence"]
        if message["action"] == "click":
            clicks += 1
        elif message["action"] == "open":
            send({"type": "open_document", "argument": f"клик-{clicks}"})
        for view in list(views):
            render(view)
    elif kind == "event":
        view_sessions = {view: message.get("session_dir") for view in views}
        for view in list(views):
            render(view)

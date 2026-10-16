"""Small stdlib SDK for the public Proteus client extension protocol v2."""

import json
import queue
import sys
import threading
import time
import urllib.parse
import urllib.request


class Extension:
    poll_seconds = None

    def __init__(self):
        self.server = None
        self.workspace = None
        self.session = None
        self.state = None
        self.entries = []
        self.active_document = None
        self.views = {}
        self.sequences = {}
        self.pending = {}
        self.command_error = None
        self.command_counter = 0
        self.direct = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def send(self, message):
        print(json.dumps(message, ensure_ascii=False, allow_nan=False), flush=True)

    def api(self, path, query=None, body=None, session=None, text=False):
        query = dict(query or {})
        selected = self.session if session is None else session
        if selected is not None:
            query["session_dir"] = selected
        url = self.server["origin"] + path
        if query:
            url += "?" + urllib.parse.urlencode(query)
        headers = {"Authorization": "Bearer " + self.server["token"]}
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(url, data=data, headers=headers)
        with self.direct.open(request, timeout=15) as response:
            result = response.read().decode()
        return result if text else json.loads(result)

    def badge(self, view, title, lines=(), face=None, hidden=False):
        self.send({"type": "badge", "view": view, "session_dir": self.session,
                   "hidden": hidden,
                   "title": title, "lines": list(lines) + ([self.command_error] if self.command_error else []),
                   "face": face or {"type": "icon", "tone": "muted"}})

    def set_session(self, kind, value):
        if self.session is not None:
            self.command_counter += 1
            identity = f"setting-{self.command_counter}"
            self.pending[identity] = (kind,value)
            self.command_error = None
            self.send({"type": "set_session", "id":identity, "session_dir": self.session,
                       "setting": {"type": kind, "value": value}})

    def open_document(self, argument, preview=False, focus=True):
        self.send({"type":"open_document", "argument":argument, "preview":preview, "focus":focus})

    def render(self, view, surface, argument):
        raise NotImplementedError

    def render_all(self):
        for view, (surface, argument) in list(self.views.items()):
            try:
                self.render(view, surface, argument)
            except Exception as error:
                if surface == "badge":
                    self.badge(view, "Ошибка расширения", [str(error)])
                else:
                    self.show(view, {"type": "text", "text": str(error), "tone": "danger"})

    def show(self, view, root, keys=None):
        self.send({"type": "view", "session_dir":self.session, "view": view, "root": root, "sequence":self.sequences.get(view,0), "keys":keys or {}})

    def action(self, view, action, value):
        pass

    def event(self, event):
        pass

    def handle(self, message):
        kind = message["type"]
        if kind == "hello":
            if message["protocol"] != 2:
                raise ValueError("This extension requires protocol 2")
            self.server, self.workspace = message["server"], message["workspace"]
        elif kind == "command_result":
            self.pending.pop(message["id"],None)
            if message["session_dir"] != self.session: return
            self.command_error = message.get("error")
            if message.get("config") is not None:
                if self.state is None: self.state={}
                self.state["config"]=message["config"]
            self.render_all()
        elif kind == "session_snapshot":
            self.session = message["session_dir"]
            self.state, self.entries = message["state"], message["entries"]
            self.render_all()
        elif kind == "session_update":
            if message["session_dir"] != self.session:
                return
            update = message["update"]
            if update["type"] == "state":
                self.state = update["state"]
            elif update["type"] == "entries":
                start = update["start"]
                self.entries[start:start + update["removed"]] = update["entries"]
            self.render_all()
        elif kind == "render":
            self.session = message.get("session_dir")
            self.views[message["view"]] = (message["surface"], message.get("argument"))
            self.render_all()
        elif kind == "close":
            self.views.pop(message["view"], None)
        elif kind == "event":
            self.active_document = message.get("document")
            if message["event"] == "session_opened":
                if message.get("session_dir") != self.session:
                    self.state, self.entries = None, []
                    self.pending.clear()
                    self.command_error = None
                self.session = message.get("session_dir")
            self.event(message["event"])
            self.render_all()
        elif kind == "action":
            self.sequences[message["view"]] = message["sequence"]
            self.action(message["view"], message["action"], message.get("value"))
            self.render_all()

    def run(self):
        inbox = queue.Queue()
        def read():
            try:
                for line in sys.stdin:
                    inbox.put(json.loads(line))
            finally:
                inbox.put(None)
        threading.Thread(target=read, daemon=True).start()
        while True:
            try:
                message = inbox.get(timeout=self.poll_seconds if self.views else None)
            except queue.Empty:
                self.render_all()
                continue
            if message is None:
                return
            self.handle(message)


def tokens(value):
    if value is None: return "—"
    if value < 1000:
        return str(value)
    if value < 1000000:
        return f"{value / 1000:.0f}k"
    return f"{value / 1000000:.1f}M"


def text(value, tone="default"):
    return {"type": "text", "text": str(value), "tone": tone}


def stack(*children, direction="column"):
    return {"type": "stack", "direction": direction, "children": list(children)}


def button(label, action, value=None):
    return {"type": "button", "label": label, "action": action, "value": value}


def select(identity, label, choices, selected, action):
    return {"type":"select", "id":identity, "label":str(label), "action":action,
            "options":[{"label":str(label), "value":value, "selected":value == selected} for value,label in choices]}


def metric(label, value, detail=None):
    return {"type":"metric", "label":label, "value":str(value), "detail":detail}


def code(identity, value, height=280):
    return {"type":"code", "id":identity, "text":json.dumps(value,ensure_ascii=False,indent=2), "language":"json", "height":height}


class Analytics(Extension):
    poll_seconds = 5

    def __init__(self):
        super().__init__()
        self.chosen = None
        self.cached = {}

    @property
    def selected_session(self):
        return self.chosen or self.session

    def fetch(self, path, query=None, active=False):
        session = self.session if active else self.selected_session
        key = (path, session, json.dumps(query,sort_keys=True))
        cached = self.cached.get(key)
        if cached is None or time.monotonic() - cached[0] >= self.poll_seconds:
            value = self.api(path, query, session=session)
            self.cached[key] = (time.monotonic(), value)
        return self.cached[key][1]

    def session_picker(self):
        sessions = self.fetch("/sessions")
        choices = [(session["session_dir"], (session.get("preview") or "Новая сессия")[:60]
                    + f" · {session['message_count']} сообщ.") for session in sessions]
        title = next((label for value,label in choices if value == self.selected_session), "Сохранённая сессия")
        return select("session",title,choices,self.selected_session,"session")

    def action(self, view, action, value):
        if action == "session":
            self.chosen = value
            self.cached.clear()
        elif action == "refresh":
            self.cached.clear()

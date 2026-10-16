"""Model quota extension. Only stdio UI protocol and the public agent API."""

import datetime
import json
import queue
import sys
import threading
import time
import urllib.parse
import urllib.request

REFRESH_SECONDS = 60
WEEK_SECONDS = 604800
DIRECT = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def number(value):
    return f"{value:.1f}".rstrip("0").rstrip(".").replace(".", ",")


def remaining(used):
    return max(0, min(100, 100 - used))


def duration(seconds, fallback):
    if seconds is None:
        return fallback
    for size, label in [(86400, "дн."), (3600, "ч"), (60, "мин")]:
        if seconds > 0 and seconds % size == 0:
            return f"{seconds // size} {label}"
    return f"{seconds} с"


def local_time(timestamp):
    return datetime.datetime.fromtimestamp(timestamp).strftime("%d.%m %H:%M")


def reset_label(timestamp, now):
    if timestamp is None:
        return "Время сброса не указано"
    if timestamp <= now:
        return "Время сброса прошло · ожидаем новые данные"
    return f"Сброс {local_time(timestamp)}"


def describe(snapshot, now):
    lines = []
    weekly = []
    if snapshot.get("plan") is not None:
        lines.append(f"План: {snapshot['plan']}")
    for bucket in snapshot["buckets"]:
        name = bucket.get("name") or bucket["id"]
        if bucket.get("limit_reached") is True:
            lines.append(f"{name}: лимит исчерпан")
        elif bucket.get("allowed") is False:
            lines.append(f"{name}: использование недоступно")
        for window in bucket["windows"]:
            left = remaining(window["used_percent"])
            if window.get("duration_seconds") == WEEK_SECONDS:
                weekly.append(left)
            label = duration(window.get("duration_seconds"), window["id"])
            lines.append(f"{name} · {label}: осталось {number(left)}% · "
                         + reset_label(window.get("resets_at"), now))
    credits = snapshot.get("credits")
    if credits is not None:
        if credits["unlimited"]:
            value = "без ограничений"
        elif credits.get("balance") is not None:
            value = credits["balance"]
        else:
            value = "доступны" if credits["available"] else "нет доступных"
        lines.append(f"Кредиты: {value}")
    lines.append(f"Данные на {local_time(snapshot['observed_at'])}")
    return (min(weekly) if weekly else None), lines


class Quota:
    def __init__(self, send):
        self.send = send
        self.server = None
        self.session = None
        self.views = set()
        self.loaded = False
        self.snapshot = None
        self.error = None
        self.deadline = 0

    def present(self):
        title, lines = "Лимиты модели", []
        face = {"type": "icon", "tone": "muted"}
        if self.loaded and self.snapshot is not None:
            left, lines = describe(self.snapshot, time.time())
            if left is not None:
                title = f"Лимиты: осталось {number(left)}%"
                face = {"type": "ring", "percent": 100 - left,
                        "tone": "danger" if left <= 10 else "warning" if left <= 25 else "muted"}
        if self.error is not None:
            lines = [f"Не удалось получить: {self.error}"]
        elif self.session is None:
            lines = ["Нет активной сессии."]
        elif not self.loaded:
            lines = ["Загрузка…"]
        elif self.snapshot is None:
            lines = ["Провайдер не сообщает лимиты."]
        for view in sorted(self.views):
            self.send({"type": "badge", "view": view, "session_dir": self.session,
                       "title": title, "lines": lines, "face": face})

    def refresh(self):
        if not self.views or not self.server or not self.session:
            return
        self.present()
        query = urllib.parse.urlencode({"session_dir": self.session})
        request = urllib.request.Request(self.server["origin"] + "/model/quota?" + query,
                                        headers={"Authorization": "Bearer " + self.server["token"]})
        try:
            with DIRECT.open(request, timeout=10) as response:
                self.snapshot = json.load(response)
            self.loaded, self.error = True, None
        except Exception as error:
            self.error = str(error)
        self.deadline = time.monotonic() + REFRESH_SECONDS
        self.present()

    def handle(self, message):
        kind = message["type"]
        if kind == "hello":
            if message["protocol"] != 2:
                raise ValueError("This extension requires protocol 2")
            self.server = message["server"]
        elif kind == "close":
            self.views.discard(message["view"])
        elif kind in ("render", "event"):
            if kind == "event" and message["event"] not in ("session_opened", "run_settled"):
                return
            session = message.get("session_dir")
            changed = session != self.session
            if changed:
                self.session = session
                self.loaded, self.snapshot, self.error, self.deadline = False, None, None, 0
            if kind == "render":
                if message["surface"] != "badge":
                    raise ValueError("Quota only declares a badge surface")
                self.views.add(message["view"])
            if changed or kind == "event" or time.monotonic() >= self.deadline:
                self.refresh()
            self.present()


def main():
    def send(message):
        print(json.dumps(message, ensure_ascii=False, allow_nan=False), flush=True)

    inbox = queue.Queue()

    def read():
        try:
            for line in sys.stdin:
                inbox.put(json.loads(line))
        finally:
            inbox.put(None)

    threading.Thread(target=read, daemon=True).start()
    quota = Quota(send)
    while True:
        timeout = max(0.05, quota.deadline - time.monotonic()) if quota.views and quota.session else None
        try:
            message = inbox.get(timeout=timeout)
        except queue.Empty:
            quota.refresh()
            continue
        if message is None:
            return
        quota.handle(message)


if __name__ == "__main__":
    main()

"""Real extension process against a disposable public-API fixture."""

import contextlib
import http.server
import importlib.util
import json
from pathlib import Path
import queue
import shutil
import subprocess
import tempfile
import threading
import unittest
import urllib.parse

PACKAGE = Path(__file__).resolve().parents[1] / "packages/quota"
spec = importlib.util.spec_from_file_location("quota", PACKAGE / "main.py")
quota = importlib.util.module_from_spec(spec)
spec.loader.exec_module(quota)


class Api(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        query = urllib.parse.urlsplit(self.path)
        session = urllib.parse.parse_qs(query.query)["session_dir"][0]
        if query.path != "/model/quota" or self.headers.get("Authorization") != "Bearer fixture":
            self.send_error(403)
            return
        body = None if session == "/empty" else {
            "observed_at": 1700000000, "plan": "test",
            "buckets": [{"id": "main", "windows": [
                {"id": "weekly", "duration_seconds": 604800, "used_percent": 92.5},
                {"id": "other", "duration_seconds": 604800, "used_percent": 80},
            ]}],
        }
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(body).encode())

    def log_message(self, *_):
        pass


class QuotaTests(unittest.TestCase):
    def test_window_formatting_and_missing_provider(self):
        self.assertEqual(quota.duration(604800, "w"), "7 дн.")
        self.assertEqual(quota.duration(18000, "w"), "5 ч")
        self.assertEqual(quota.duration(90, "w"), "90 с")
        self.assertEqual(quota.duration(None, "primary"), "primary")
        self.assertEqual(quota.number(quota.remaining(92.5)), "7,5")
        self.assertEqual(quota.remaining(130), 0)
        self.assertEqual(quota.reset_label(None, 10), "Время сброса не указано")
        self.assertIn("прошло", quota.reset_label(5, 10))

    def test_process_api_session_switch_and_independent_update(self):
        with contextlib.ExitStack() as stack:
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Api)
            threading.Thread(target=server.serve_forever, daemon=True).start()
            stack.callback(server.server_close)
            stack.callback(server.shutdown)
            directory = Path(stack.enter_context(tempfile.TemporaryDirectory())) / "quota"
            shutil.copytree(PACKAGE, directory)

            def run():
                process = subprocess.Popen(["python3", "main.py"], cwd=directory,
                                           stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                           stderr=subprocess.PIPE, text=True)
                def stop():
                    if process.poll() is None:
                        process.terminate()
                    process.communicate(timeout=5)
                stack.callback(stop)
                messages = queue.Queue()
                def read():
                    for line in process.stdout:
                        messages.put(json.loads(line))
                threading.Thread(target=read, daemon=True).start()
                def send(message):
                    process.stdin.write(json.dumps(message) + "\n")
                    process.stdin.flush()
                send({"type": "hello", "protocol": 2, "workspace": "/tmp",
                      "server": {"origin": f"http://127.0.0.1:{server.server_port}", "token": "fixture"}})
                return process, messages, send

            process, messages, send = run()
            send({"type": "render", "view": "badge", "surface": "badge", "session_dir": "/a"})
            while True:
                badge = messages.get(timeout=5)
                if badge["face"]["type"] == "ring":
                    break
            self.assertEqual(badge["title"], "Лимиты: осталось 7,5%")
            self.assertEqual(badge["face"], {"type": "ring", "percent": 92.5, "tone": "danger"})
            self.assertTrue(any("7 дн." in line for line in badge["lines"]))
            send({"type": "event", "event": "session_opened", "session_dir": "/empty"})
            while True:
                badge = messages.get(timeout=5)
                if badge["session_dir"] == "/empty" and "Провайдер не сообщает лимиты." in badge["lines"]:
                    break
            self.assertEqual(badge["face"]["type"], "icon")
            process.terminate()
            process.wait(timeout=5)

            # Replace only the package; the identical host exchange now sees
            # the changed title without building or changing any client code.
            path = directory / "main.py"
            path.write_text(path.read_text().replace("Лимиты модели", "Updated quota"))
            _, messages, send = run()
            send({"type": "render", "view": "badge", "surface": "badge", "session_dir": None})
            self.assertEqual(messages.get(timeout=5)["title"], "Updated quota")


if __name__ == "__main__":
    unittest.main()

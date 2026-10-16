import sys
import time
from pathlib import Path, PurePosixPath
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdk"))
from proteus_extension import Extension, stack, text, button

MARKS = {"conflict":"!", "untracked":"U", "renamed":"R", "deleted":"D", "added":"A", "modified":"M"}
TONES = {"conflict":"danger", "deleted":"danger", "untracked":"success", "added":"success", "renamed":"warning", "modified":"warning"}
from tree import search_rows, language


class Files(Extension):
    poll_seconds = 5

    def __init__(self):
        super().__init__()
        self.query = ""
        self.tab = "tree"
        self.expanded = {""}
        self.listings = {}
        self.selected = None
        self.changes = {"entries":[], "repository":False, "truncated":False}
        self.deadline = 0
        self.truncated = False
        self.documents = {}
        self.modes = {}

    def event(self, event):
        if event == "navigation_changed" and self.active_document:
            self.selected = self.active_document
            self.expanded.update(str(parent) for parent in PurePosixPath(self.active_document).parents if str(parent) != ".")
        if event == "session_opened":
            self.listings.clear()
            self.expanded = {""}
            self.selected = None
        if event in ("session_opened", "run_settled"):
            self.documents.clear()
            self.deadline = 0

    def refresh(self):
        if time.monotonic() >= self.deadline:
            self.changes = self.api("/workspace/changes")
            self.listings.clear()
            self.deadline = time.monotonic() + self.poll_seconds

    def listing(self, path):
        if path not in self.listings:
            self.listings[path] = self.api("/workspace/list", {"path":path})
        return self.listings[path]

    def row(self, path, label, depth=0, directory=False):
        status = next((item["status"] for item in self.changes["entries"] if item["path"] == path), None)
        descendants = directory and any(item["path"].startswith(path + "/") for item in self.changes["entries"])
        return {"id":path, "label":label, "detail":MARKS.get(status, "•" if descendants else ""), "depth":depth,
                "expanded":path in self.expanded if directory else None,
                "tone":TONES.get(status, "warning" if descendants else "muted" if directory else "default"), "selected":path == self.selected}

    def rows(self):
        self.truncated = self.changes["truncated"]
        if self.tab == "changes":
            return [self.row(item["path"], item["path"]) for item in self.changes["entries"]]
        if self.query:
            result = self.api("/workspace/search", {"q":self.query})
            self.truncated |= result["truncated"]
            return search_rows(result, self.row)
        rows = []
        def visit(path, depth):
            listing = self.listing(path)
            self.truncated |= listing["truncated"]
            for entry in listing["entries"]:
                directory = entry["kind"] == "directory"
                rows.append(self.row(entry["path"], entry["name"], depth, directory))
                if directory and entry["path"] in self.expanded:
                    visit(entry["path"], depth + 1)
        visit("", 0)
        return rows

    def render(self, view, surface, argument):
        if not self.session:
            if surface == "badge": self.badge(view, "Файлы", ["Нет активной сессии."])
            else: self.show(view, text("Нет активной сессии.", "muted"))
            return
        self.refresh()
        if surface == "badge":
            entries = self.changes["entries"]
            self.badge(view, f"Изменённых файлов: {len(entries)}", [f"{MARKS.get(entry['status'],'')} {entry['path']}" for entry in entries],
                       {"type":"icon", "count":len(entries), "tone":"muted"} if entries else None)
        elif surface == "panel":
            controls = stack(button("Файлы", "tab", "tree"), button("Изменения", "tab", "changes"), button("↻", "refresh"), direction="row")
            rows = self.rows()
            warning = [text("Список ограничен сервером; уточните поиск.", "warning")] if self.truncated else []
            self.show(view, {"type":"fill", "children":[controls, *warning,
                {"type":"input", "id":"search", "value":self.query, "placeholder":"Поиск по проекту…", "action":"search"},
                {"type":"tree", "id":"files", "rows":rows, "action":"file"}]}, keys={"ctrl-f":"search-focus", "/":"search-focus", "escape":"escape"})
        elif surface == "document":
            mode = self.modes.get(argument, "file")
            key = (argument, mode)
            if key not in self.documents:
                self.documents[key] = self.api("/workspace/diff" if mode == "diff" else "/workspace/file", {"path":argument})
            document = self.documents[key]
            content = document.get("patch") if mode == "diff" else document.get("text")
            syntax = "diff" if mode == "diff" else language(argument)
            body = {"type":"code", "id":"content", "text":content, "language":syntax} if content is not None else text({"binary":"Двоичный файл", "too_large":"Файл превышает предел предпросмотра 512 КБ", "unavailable":"Изменений нет"}.get(document["kind"], "Нет текстового содержимого"), "muted")
            header = stack(text(argument), button("Файл", "document-mode", [argument,"file"]),
                           button("Изменения", "document-mode", [argument,"diff"]), button("Копировать путь", "copy", argument), direction="row")
            self.show(view, {"type":"fill", "children":[header, body]})

    def action(self, view, action, value):
        if action == "search-focus":
            self.send({"type":"focus", "view":view, "control":"search"})
        elif action == "escape":
            if self.query:
                self.query = ""
                control = "files"
            else: control = None
            self.send({"type":"focus", "view":view, "control":control})
        elif action == "search":
            self.query = value
        elif action == "tab":
            self.tab = value
        elif action == "refresh":
            self.deadline = 0
            self.listings.clear()
            self.documents.clear()
        elif action == "copy":
            self.send({"type":"clipboard", "text":value})
        elif action == "document-mode":
            self.modes[value[0]] = value[1]
        elif action == "file":
            path, event = value["id"], value["event"]
            directory = any(row["id"] == path and row["expanded"] is not None for row in self.rows())
            self.selected = path
            if directory:
                if self.query:
                    self.query = ""
                    self.expanded.update(str(parent) for parent in PurePosixPath(path).parents if str(parent) != ".")
                if event in ("select", "open"):
                    self.expanded.symmetric_difference_update({path})
                elif event == "expand": self.expanded.add(path)
                elif event == "collapse": self.expanded.discard(path)
            elif event in ("select", "preview", "open"):
                self.modes[path] = "diff" if self.tab == "changes" else self.modes.get(path, "file")
                self.open_document(path, preview=event != "open", focus=event == "open")


if __name__ == "__main__":
    Files().run()

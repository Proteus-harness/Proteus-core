#!/usr/bin/env python3
"""Bounded JSONL stdio MCP issue service; all data is synthetic and local."""
import argparse
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import queue
import sys
import threading

from fixtures import read_json, write_json
from schemas import declared_tools


def tool(name, description, properties, required, readonly):
    return {"name": name, "description": description,
            "inputSchema": {"type": "object", "properties": properties,
                            "required": required, "additionalProperties": False},
            "annotations": {"readOnlyHint": readonly, "destructiveHint": not readonly}}


TOOLS = [
    tool("list_issues", "List stable issue IDs, at most 8 per page. Start with cursor=null; follow next_cursor until null.",
         {"cursor": {"type": ["string", "null"]},
          "limit": {"type": "integer", "minimum": 1, "maximum": 8}}, ["cursor"], True),
    tool("get_issue", "Fetch the complete JSON issue by ID. Required for filtering; list pages contain only IDs.",
         {"id": {"type": "string"}}, ["id"], True),
    tool("update_queue", "Change only an issue's queue. Fetch the issue again after updating to verify all other fields.",
         {"id": {"type": "string"}, "queue": {"type": "string", "enum": ["Backlog"]}},
         ["id", "queue"], False),
    tool("submit_report", "Persist the final report JSON once. Returns an acknowledgement, not a correctness oracle.",
         {"report": {"type": "object", "additionalProperties": True}}, ["report"], False),
]


class IssueService:
    def __init__(self, directory):
        self.directory = Path(directory)
        self.state = read_json(self.directory / "current.json")
        self.issues = {issue["id"]: issue for issue in self.state["issues"]}
        self.cursors = {self._cursor(offset): offset for offset in range(1, len(self.issues))}
        self.lock = threading.Lock()
        self.ordinal = sum(bool(line.strip()) for line in (self.directory / "api.jsonl").read_text().splitlines())

    def _cursor(self, offset):
        text = f"{self.state['seed']}:{self.state['case']}:page:{offset}"
        return "cursor_" + hashlib.sha256(text.encode()).hexdigest()[:16]

    def invoke(self, name, arguments):
        with self.lock:
            try:
                value = self._invoke(name, arguments)
                outcome = {"ok": True, "result": value}
            except (ValueError, KeyError, TypeError) as error:
                outcome = {"ok": False, "result": {"error": str(error)}}
            self.ordinal += 1
            event = {"ordinal": self.ordinal, "method": name,
                     "params": arguments, "outcome": outcome}
            with (self.directory / "api.jsonl").open("a", encoding="utf-8") as file:
                file.write(json.dumps(event, ensure_ascii=False, sort_keys=True) + "\n")
            return {"content": [], "structuredContent": outcome["result"], "isError": not outcome["ok"]}

    def _invoke(self, name, arguments):
        if not isinstance(arguments, dict):
            raise ValueError("arguments must be an object")
        definitions = {entry["name"]: entry["inputSchema"] for entry in TOOLS}
        if name not in definitions:
            raise ValueError(f"unknown tool: {name}")
        schema = definitions[name]
        if set(arguments) - set(schema["properties"]) or set(schema["required"]) - set(arguments):
            raise ValueError("unknown or missing argument")
        if name == "list_issues":
            cursor, limit = arguments["cursor"], arguments.get("limit", 8)
            if cursor is not None and (not isinstance(cursor, str) or cursor not in self.cursors):
                raise ValueError("unknown cursor")
            if type(limit) is not int or not 1 <= limit <= 8:
                raise ValueError("limit must be an integer in 1..8")
            offset = 0 if cursor is None else self.cursors[cursor]
            page = self.state["issues"][offset:offset + limit]
            end = offset + len(page)
            return {"issues": [{"id": issue["id"]} for issue in page],
                    "next_cursor": self._cursor(end) if end < len(self.issues) else None}
        if name == "get_issue":
            return deepcopy(self.issues[arguments["id"]])
        if name == "update_queue":
            if arguments["queue"] != "Backlog":
                raise ValueError("only the Backlog destination is supported")
            issue = self.issues[arguments["id"]]
            issue["queue"] = arguments["queue"]
            write_json(self.directory / "current.json", self.state)
            return {"id": issue["id"], "queue": issue["queue"]}
        if not isinstance(arguments["report"], dict):
            raise ValueError("report must be an object")
        write_json(self.directory / "report.json", arguments["report"])
        return {"stored": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", type=Path, required=True)
    parser.add_argument("--api-schemas", action="store_true")
    args = parser.parse_args()
    service = IssueService(args.state_dir)
    tools = declared_tools(TOOLS, service.state["case"]) if args.api_schemas else TOOLS
    output_lock = threading.Lock()
    pending = queue.Queue(maxsize=128)

    def send(value):
        with output_lock:
            print(json.dumps(value, ensure_ascii=False, separators=(",", ":")), flush=True)

    def handle(request):
        request_id = request.get("id")
        if "id" not in request:
            return  # Notifications, including initialized/cancelled, have no reply.
        method, params = request.get("method"), request.get("params") or {}
        if method == "initialize":
            value = {"protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                     "serverInfo": {"name": "codemode-issue-lab", "version": "1"}}
        elif method == "ping":
            value = {}
        elif method == "tools/list":
            value = {"tools": tools}
        elif method == "tools/call":
            value = service.invoke(params.get("name"), params.get("arguments", {}))
        else:
            send({"jsonrpc": "2.0", "id": request_id,
                  "error": {"code": -32601, "message": f"unknown method: {method}"}})
            return
        send({"jsonrpc": "2.0", "id": request_id, "result": value})

    def worker():
        while True:
            request = pending.get()
            try:
                if request is None:
                    return
                try:
                    handle(request)
                except Exception as error:
                    send({"jsonrpc": "2.0", "id": request.get("id"),
                          "error": {"code": -32603, "message": f"fixture error: {type(error).__name__}"}})
            finally:
                pending.task_done()

    workers = [threading.Thread(target=worker) for _ in range(8)]
    for thread in workers:
        thread.start()
    try:
        for line in sys.stdin:
            try:
                request = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(request, dict):
                pending.put(request)
    finally:
        for _ in workers:
            pending.put(None)
        for thread in workers:
            thread.join()


if __name__ == "__main__":
    main()

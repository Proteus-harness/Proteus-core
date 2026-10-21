#!/usr/bin/env python3
"""Stateful newline-delimited MCP fixture used by mcp_client.rs."""

import json
import os
import sys
import threading
import time
import socket
import subprocess


EVENT_LOG = os.environ["MCP_FIXTURE_EVENT_LOG"]
GENERATION_FILE = os.environ["MCP_FIXTURE_GENERATION_FILE"]
INIT_MODE = os.environ.get("MCP_FIXTURE_INIT_MODE", "ok")
WRITE_LOCK = threading.Lock()
CANCEL_LOCK = threading.Lock()
CANCELLED = {}
SANDBOX = os.environ.get("MCP_FIXTURE_SANDBOX") == "1"


def append(path, line):
    with open(path, "a", encoding="utf-8") as file:
        file.write(line + "\n")
        file.flush()


with open(GENERATION_FILE, "a+", encoding="utf-8") as generations:
    generations.seek(0)
    GENERATION = sum(1 for line in generations if line.strip()) + 1
    generations.write(f"{GENERATION}\n")
    generations.flush()


def event(name):
    append(EVENT_LOG, f"{GENERATION}:{name}")


def send(message):
    encoded = json.dumps(message, ensure_ascii=False, separators=(",", ":"))
    with WRITE_LOCK:
        sys.stdout.write(encoded + "\n")
        sys.stdout.flush()


def result(request_id, value):
    send({"jsonrpc": "2.0", "id": request_id, "result": value})


def error(request_id, code, message):
    send({"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}})


def tool(name, description, properties=None):
    return {
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties or {},
            "additionalProperties": False,
        },
    }


def call_tool(request_id, params):
    name = params.get("name")
    arguments = params.get("arguments", {})
    event(f"call_started:{request_id}:{name}")
    if name == "echo":
        if arguments.get("mode") == "rpc_error":
            error(request_id, -32001, "ordinary fixture RPC error")
            event(f"call_finished:{request_id}:{name}")
            return
        delay_ms = arguments.get("delay_ms", 0)
        time.sleep(delay_ms / 1000)
        label = arguments.get("label", "")
        mode = arguments.get("mode", os.environ.get("MCP_FIXTURE_CONTENT_MODE", "mixed"))
        payload = {
            "content": [{"type": "text", "text": f"echo:{label}:generation:{GENERATION}"}],
            "structuredContent": {
                "label": label,
                "generation": GENERATION,
                "path_present": bool(os.environ.get("PATH")),
                "home_present": bool(os.environ.get("HOME")),
                "literal_env": os.environ.get("MCP_LITERAL_ENV"),
            },
            "isError": False,
        }
        if mode == "structured_only":
            payload["content"] = []
        elif mode == "null_structured":
            payload["structuredContent"] = None
        elif mode == "text_only":
            del payload["structuredContent"]
        elif mode == "empty_structured":
            payload["structuredContent"] = {}
        result(request_id, payload)
    elif name == "fail":
        result(request_id, {
            "content": [{"type": "text", "text": "fixture failure"}],
            "structuredContent": {"kind": "expected"},
            "isError": True,
        })
    elif name == "delay_write":
        delay_ms = arguments.get("delay_ms", 1000)
        deadline = time.monotonic() + delay_ms / 1000
        key = json.dumps(request_id, sort_keys=True)
        while time.monotonic() < deadline:
            with CANCEL_LOCK:
                if CANCELLED.get(key, False):
                    event(f"call_cancelled:{request_id}")
                    error(request_id, -32800, "request cancelled")
                    return
            time.sleep(0.01)
        append(arguments["marker"], f"generation:{GENERATION}")
        result(request_id, {"content": [{"type": "text", "text": "written"}], "isError": False})
    elif SANDBOX and name == "sandbox_probe":
        def denied(operation):
            try:
                operation()
            except OSError:
                return True
            return False

        def write(path):
            with open(path, "w", encoding="utf-8") as file:
                file.write("sandbox effect\n")

        with open("allowed.txt", encoding="utf-8") as file:
            readable = file.read() == "synthetic workspace\n"
        write(arguments["allowed_write"])
        with socket.socket() as connection:
            connection.settimeout(0.3)
            connected = connection.connect_ex(("127.0.0.1", arguments["port"])) == 0
        checks = {
            "workspace_read": readable,
            "workspace_readonly": denied(lambda: write("denied-write.txt")),
            "private_read_denied": denied(lambda: open(arguments["private_read"]).close()),
            "symlink_read_denied": denied(lambda: open("secret-link").close()),
            "private_write_denied": denied(lambda: write(arguments["private_write"])),
            "network_connected": connected,
            "home_absent": "HOME" not in os.environ,
            "literal_env": os.environ.get("MCP_LITERAL_ENV"),
            "cwd": os.getcwd(),
        }
        result(request_id, {"content": [], "structuredContent": checks, "isError": False})
    elif SANDBOX and name == "spawn_descendant":
        # Deliberately ignore request cancellation. A setsid child continues
        # independently unless the owning MCP generation is actually torn down.
        code = '''import os,sys,time
ready,heartbeat,marker=sys.argv[1:]
with open(heartbeat,"w") as output: output.write("started\\n")
open(ready,"w").write("ready")
for _ in range(40):
    with open(heartbeat,"a") as output: output.write("tick\\n")
    time.sleep(0.05)
open(marker,"w").write("late effect")
'''
        subprocess.Popen([sys.executable, "-B", "-c", code, arguments["ready"],
                          arguments["heartbeat"], arguments["marker"]],
                         start_new_session=True)
        time.sleep(30)
    elif name == "oversize":
        result(request_id, {"content": [{"type": "text", "text": "x" * 100000}], "isError": False})
    else:
        error(request_id, -32602, f"unknown tool {name}")
    event(f"call_finished:{request_id}:{name}")


event("spawn")
for raw_line in sys.stdin:
    try:
        request = json.loads(raw_line)
    except json.JSONDecodeError:
        continue
    method = request.get("method")
    request_id = request.get("id")
    params = request.get("params") or {}
    if method == "initialize":
        event("initialize")
        if INIT_MODE == "malformed":
            result(request_id, {"protocolVersion": 42})
        elif INIT_MODE == "wrong_version":
            result(request_id, {
                "protocolVersion": "1900-01-01",
                "capabilities": {},
                "serverInfo": {"name": "fixture", "version": "1"},
            })
        else:
            result(request_id, {
                "protocolVersion": params.get("protocolVersion", "2025-11-25"),
                "capabilities": {"tools": {"listChanged": False}},
                "serverInfo": {"name": "fixture", "version": "1"},
            })
    elif method == "notifications/initialized":
        event("initialized")
    elif method == "tools/list":
        cursor = params.get("cursor")
        event(f"list:{cursor or 'first'}")
        if cursor is None:
            result(request_id, {
                "tools": [tool("echo", "echo a label", {
                    "label": {"type": "string"},
                    "delay_ms": {"type": "integer"},
                    "mode": {"type": "string"},
                }), tool("fail", "return an MCP tool error")],
                "nextCursor": "page-2",
            })
        else:
            tools = [
                tool("delay_write", "write after a delay", {
                    "marker": {"type": "string"},
                    "delay_ms": {"type": "integer"},
                }),
                tool("oversize", "return a response over the configured bound"),
            ]
            if SANDBOX:
                tools.extend([tool("sandbox_probe", "probe explicit OS grants"),
                              tool("spawn_descendant", "spawn a cancellation-resistant child")])
            result(request_id, {"tools": tools})
    elif method == "tools/call":
        thread = threading.Thread(target=call_tool, args=(request_id, params), daemon=True)
        thread.start()
    elif method == "notifications/cancelled":
        cancelled_id = params.get("requestId")
        event(f"cancel_notification:{cancelled_id}")
        with CANCEL_LOCK:
            CANCELLED[json.dumps(cancelled_id, sort_keys=True)] = True
    elif request_id is not None:
        error(request_id, -32601, f"unsupported method {method}")

event("eof")

#!/usr/bin/env python3
"""Freeform tool component with a non-idempotent, barrier-controlled effect."""

import os
import socket
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[5] / "examples/modules"))
from component_runtime import PROTOCOL_VERSION, ProtocolError, run_component

EXPORT = {
    "slot": "tool", "module_id": "crash-probe", "contract_version": "v6",
    "composition": "ordered_many", "module_features": [], "config_schema": None,
}
SPEC = {
    "name": "crash_probe",
    "description": "Write the fixture files from a custom tool invocation.",
    "input_schema": {"type": "object", "properties": {
        "input": {"type": "string"}}, "required": ["input"],
        "additionalProperties": False},
    "surface": {"kind": "freeform", "format": {
        "type": "grammar", "syntax": "lark", "definition": "start: /[a-z ]+/"}},
    "safety": "RunsCommands", "supports_parallel_tool_calls": False,
    "timeout_ms": 20000, "metadata": {},
}


def initialize(params):
    if (params.get("protocol_version") != PROTOCOL_VERSION
            or params.get("component_id") != "crash-probe-component"):
        raise ProtocolError("invalid crash probe initialization")
    exports = params.get("exports") or []
    if [(item.get("slot"), item.get("module_id"),
         item.get("contract_version"), item.get("composition"))
            for item in exports] != [("tool", "crash-probe", "v6", "ordered_many")]:
        raise ProtocolError("unexpected crash probe exports")
    return {"protocol_version": PROTOCOL_VERSION,
            "component_id": params["component_id"], "exports": [EXPORT]}


def invoke(context, method, params):
    if context.export != {"slot": "tool", "module_id": "crash-probe"}:
        raise ProtocolError("unexpected crash probe export")
    if method == "list":
        return {"result": [{"spec": SPEC, "model_visible": True, "user_command": None}]}
    if method != "invoke":
        raise ProtocolError("unexpected crash probe method")
    call = params["call"]
    if (call["name"] != "crash_probe" or call["surface"] != "freeform"
            or call["args"] != {"input": "change files"}):
        raise ProtocolError("unexpected freeform crash probe input")
    context.ensure_active()
    directory = Path(params["cwd"])
    # Close each file before acknowledging the effect barrier. An invocation
    # repeated during cold resume would append another x and fail the test.
    with (directory / "effects.log").open("a") as output:
        output.write("x")
    for name in "abc":
        (directory / f"{name}.txt").write_text(name)
    if os.environ["CRASH_PROBE_WAIT"] == "true":
        with socket.socket(socket.AF_UNIX) as barrier:
            context.on_cancel(barrier.close)
            barrier.connect(str(directory / "crash-gate.sock"))
            barrier.recv(1)
        context.ensure_active()
    return {"result": {"call_id": call["id"], "ok": True, "output": "",
                       "content": [], "error": None, "metadata": {}}}


run_component(initialize, invoke)

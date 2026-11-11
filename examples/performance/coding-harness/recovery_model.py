#!/usr/bin/env python3
"""Synthetic model/v13 for recovery proofs, with real Core and tool processes."""
import json
from pathlib import Path
import sys
import uuid

# The runner mounts the existing example protocol runtime beside this module.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from component_runtime import run_component

CONFIG = {}
ATTEMPT = 0
PREVIOUS = None
CALL_ID = "recovery-append-once"


def capabilities():
    return {
        "adapter_id": "recovery-probe", "hosted_tools": [],
        "capabilities": {
            **dict.fromkeys(("supports_tools", "supports_parallel_tool_calls", "supports_streaming",
                             "supports_system_role", "supports_developer_role", "supports_cache_hints",
                             "supports_reasoning_config"), True),
            **dict.fromkeys(("supports_freeform_tools", "supports_json_schema", "supports_image_input",
                             "supports_file_input"), False),
            "provider_hosted_tools": [], "max_input_tokens": 1000000, "max_output_tokens": 384000,
        },
    }


def initialize(params):
    binding, = params["exports"]
    assert binding["slot"] == "model" and binding["module_id"] == "recovery-probe"
    assert binding["contract_version"] == "v13"
    CONFIG.update(binding["module_config"])
    return {"protocol_version": "v3", "component_id": params["component_id"], "exports": [{
        "slot": "model", "module_id": "recovery-probe", "contract_version": "v13",
        "composition": "select_one", "module_features": [], "config_schema": None,
    }]}


def message(payload):
    return {"id": str(uuid.uuid4()), "role": "Assistant", "phase": None, "name": None,
            "tool_call_id": None, "metadata": None,
            "parts": [{"part_id": str(uuid.uuid4()), "provenance": "model", "scope": "conversation", "payload": payload}]}


def append_command():
    code = "from pathlib import Path; p=Path('counter.txt'); p.write_text(str(int(p.read_text())+1)); print('effect-count='+p.read_text())"
    # Avoid invoking a shell parser on a generated command string.
    import shlex
    return "python3 -c " + shlex.quote(code)


def invoke(context, method, params):
    global ATTEMPT, PREVIOUS
    if method == "describe":
        return capabilities()
    if method in ("catalog", "quota"):
        return None
    assert method == "stream", method
    ATTEMPT += 1
    request = params["request"]
    tool = CONFIG["tool"]
    assert tool in [item["name"] for item in request["tools"]]
    capture = Path(CONFIG["capture"])
    with capture.open("a") as stream:
        stream.write(json.dumps({"attempt": ATTEMPT, "request": request}) + "\n")
    if ATTEMPT == 1:
        args = {"cmd" if tool == "exec_command" else "command": append_command()}
        if tool == "shell":
            args.update(description="Increment the recovery proof counter", background=False, timeout=5000)
        call = {"id": CALL_ID, "name": tool, "args": args, "surface": "function", "raw_arguments": None}
        reply = {"messages": [message({"ToolCall": {"call": call}})], "tool_calls": [call],
                 "finish_reason": "ToolCalls", "usage": None, "end_turn": None, "provider_metadata": None}
        return {"event_count": 0, "terminal": {"kind": "response", "response": reply}}
    results = [part["payload"]["ToolResult"]["result"] for msg in request["messages"]
               for part in msg["parts"] if "ToolResult" in part["payload"]]
    assert len([result for result in results if result["call_id"] == CALL_ID]) == 1, results
    result, = [result for result in results if result["call_id"] == CALL_ID]
    assert result["ok"] and "effect-count=1" in result["output"], result
    assert Path("/workspace/counter.txt").read_text() == "1"
    if PREVIOUS is not None:
        assert request == PREVIOUS, "retry changed the already accepted sampling request"
    PREVIOUS = request
    if ATTEMPT == 2 or CONFIG.get("mode") in ("exhausted", "terminal"):
        delta_id = str(uuid.uuid4())
        events = [
            {"TextDelta": {"message_id": delta_id, "phase": None, "text": "UNCOMMITTED_DRAFT"}},
            {"ToolCallDelta": {"call_id": "recovery-partial-call", "name": tool,
                               "args_delta": '{"cmd":"touch partial-marker.txt"'}},
        ]
        for sequence, event in enumerate(events):
            context.host_call("host.model.emit", {"sequence": sequence, "event": event})
        kind = "other" if CONFIG.get("mode") == "terminal" else "stream_disconnected"
        return {"event_count": len(events), "terminal": {"kind": "stream_error", "failure": {
            "kind": kind, "message": "synthetic transport disconnected after a committed effect",
            "completed_messages": [],
        }}}
    assert ATTEMPT == 3, ATTEMPT
    reply = {"messages": [message({"Text": {"text": "RECOVERY_OK"}})], "tool_calls": [],
             "finish_reason": "Stop", "usage": None, "end_turn": True, "provider_metadata": None}
    return {"event_count": 0, "terminal": {"kind": "response", "response": reply}}


if __name__ == "__main__":
    run_component(initialize, invoke)

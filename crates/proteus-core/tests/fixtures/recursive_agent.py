"""Model-free workflow exercising nested task tools on full process peers."""
import json
import os
import sys
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "examples/modules"))
from component_runtime import ProtocolError, run_component


def initialize(params):
    exports = []
    for export in params["exports"]:
        if (export["slot"], export["module_id"], export["contract_version"]) != ("workflow", "recursive_probe", "v20"):
            raise ProtocolError("unexpected recursive workflow")
        exports.append({key: export[key] for key in ("slot", "module_id", "contract_version", "composition")}
                       | {"module_features": [], "config_schema": None})
    return {"protocol_version": "v3", "component_id": params["component_id"], "exports": exports}


def assistant(text):
    return {"id": str(uuid.uuid4()), "role": "Assistant", "phase": None,
            "parts": [{"part_id": str(uuid.uuid4()), "provenance": "model", "scope": "conversation", "payload": {"Text": {"text": text}}}],
            "name": None, "tool_call_id": None, "metadata": None}


def invoke(context, method, params):
    if method != "run":
        raise ProtocolError("unexpected method")
    task = params["task"]
    with Path(task["cwd"], "recursive-peers.jsonl").open("a") as output:
        output.write(json.dumps({"pid": os.getppid(), "session": params["runtime"]["conversation"]["session_id"]}) + "\n")
    # A negative control terminates after four peers even without the host fix.
    remaining = int(task["text"])
    if remaining == 0:
        text = "recursion fixture safety bound reached"
    else:
        call = {"id": str(uuid.uuid4()), "name": "task",
                "args": {"agent_type": "next", "description": "depth probe", "prompt": str(remaining - 1)},
                "surface": "function", "raw_arguments": None}
        result = context.host_call("host.tools.execute_batch", {"task": task, "calls": [call]})[0]
        text = result["output"] if result["ok"] else (result["error"] or result["output"])
    output = {"text": text, "metadata": None}
    context.host_call("host.events.emit", {"event": {"TurnFinished": {"output": output}}})
    return {"status": "success", "result": {"output": output, "new_messages": [assistant(text)], "history_replacement": None, "compactions": []}}


run_component(initialize, invoke)

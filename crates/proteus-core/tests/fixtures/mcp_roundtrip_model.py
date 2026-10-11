"""Model/v12 fixture calling the actual discovered MCP name, then reading its result."""
import sys
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "examples/modules"))
from component_runtime import ProtocolError, run_component

descriptor = None


def initialize(params):
    global descriptor
    export = params["exports"][0]
    if (export["slot"], export["contract_version"], export["composition"]) != ("model", "v12", "select_one"):
        raise ProtocolError("expected model/v12")
    descriptor = export["module_config"]["descriptor"]
    return {"protocol_version": "v3", "component_id": params["component_id"], "exports": [
        {**{key: export[key] for key in ("slot", "module_id", "contract_version", "composition")}, "module_features": [], "config_schema": None}
    ]}


def invoke(context, method, params):
    if method == "describe":
        return descriptor
    if method in ("catalog", "quota"):
        return None
    if method != "stream":
        raise ProtocolError("expected model stream")
    request = params["request"]
    results = [part["payload"]["ToolResult"]["result"] for message in request["messages"]
               for part in message["parts"] if "ToolResult" in part["payload"]]
    if results:
        call = None
        payload = {"Text": {"text": "MCP result:\n" + results[-1]["output"]}}
    else:
        if not any(tool["name"] == "fixture__echo" for tool in request["tools"]):
            raise ProtocolError("actual discovered MCP tool is not exposed")
        call = {"id": str(uuid.uuid4()), "name": "fixture__echo", "args": {},
                "surface": "function", "raw_arguments": None}
        payload = {"ToolCall": {"call": call}}
    message = {"id": str(uuid.uuid4()), "role": "Assistant", "phase": None,
               "name": None, "tool_call_id": None, "metadata": None,
               "parts": [{"part_id": str(uuid.uuid4()), "provenance": "model",
                          "scope": "conversation", "payload": payload}]}
    return {"event_count": 0, "terminal": {"kind": "response", "response": {
        "messages": [message], "tool_calls": [call] if call else [],
        "finish_reason": "ToolCalls" if call else "Stop", "usage": None,
        "end_turn": None, "provider_metadata": None,
    }}}


run_component(initialize, invoke)

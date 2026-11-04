"""Small model-free workflow with explicit terminal success/error paths."""
import sys
import uuid
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "examples/modules"))
from component_runtime import ProtocolError, run_component


def initialize(params):
    exports = []
    for export in params["exports"]:
        if (export["slot"], export["contract_version"], export["composition"]) != ("workflow", "v20", "select_one"):
            raise ProtocolError("expected workflow/v20")
        exports.append({key: export[key] for key in ("slot", "module_id", "contract_version", "composition")}
                       | {"module_features": [], "config_schema": None})
    return {"protocol_version": "v3", "component_id": params["component_id"], "exports": exports}


def assistant(text):
    return {"id": str(uuid.uuid4()), "role": "Assistant", "phase": None,
            "parts": [{"part_id": str(uuid.uuid4()), "provenance": "model", "scope": "conversation", "payload": {"Text": {"text": text}}}],
            "name": None, "tool_call_id": None, "metadata": None}


def invoke(_context, method, params):
    if method != "run":
        raise ProtocolError("expected run")
    if params["task"]["text"] == "fail":
        return {"status": "error", "failure": {"message": "synthetic workflow failure", "history": None, "model_failure": None}}
    return {"status": "success", "result": {"output": {"text": "completed", "metadata": None},
            "new_messages": [assistant("completed")], "history_replacement": None, "compactions": []}}


run_component(initialize, invoke)

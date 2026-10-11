"""Small tool/v6 component for reload and checkpoint revision evidence."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "examples/modules"))
from component_runtime import ProtocolError, run_component


def initialize(params):
    exports = []
    for export in params["exports"]:
        if (export["slot"], export["contract_version"], export["composition"]) != ("tool", "v6", "ordered_many"):
            raise ProtocolError("expected tool/v6 ordered_many")
        exports.append({**{key: export[key] for key in ("slot", "module_id", "contract_version", "composition")}, "module_features": [], "config_schema": None})
    return {"protocol_version": "v3", "component_id": params["component_id"], "exports": exports}


def invoke(context, method, params):
    name = context.export["module_id"]
    if method == "list":
        return {"result": [{"spec": {
            "name": name, "description": "Process probe fixture",
            "input_schema": {"type": "object"},
            "surface": {"kind": "function", "strict": False, "output_schema": None},
            "safety": "ReadOnly", "supports_parallel_tool_calls": False,
            "timeout_ms": 3000, "metadata": {},
        }, "model_visible": True, "user_command": None}]}
    if method != "invoke" or params["call"]["name"] != name:
        raise ProtocolError("expected registered probe invocation")
    attribution = params["attribution"]
    if not attribution["execution_id"]:
        raise ProtocolError("execution attribution is required")
    return {"result": {"call_id": params["call"]["id"], "ok": True,
                       "output": "child" if name == "checkpoint_probe" else "ok",
                       "content": [], "error": None, "metadata": {}}}


run_component(initialize, invoke)

"""Tool component proving that host config reaches the real OS launch path."""
import json
import os
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "examples/modules"))
from component_runtime import ProtocolError, run_component


def initialize(params):
    exports = params["exports"]
    if len(exports) != 1 or exports[0]["slot"] != "tool":
        raise ProtocolError("expected one tool export")
    export = exports[0]
    return {"protocol_version": "v3", "component_id": params["component_id"],
            "exports": [{**{key: export[key] for key in
                            ("slot", "module_id", "contract_version", "composition")},
                         "module_features": [], "config_schema": None}]}


def invoke(context, method, params):
    if method == "list":
        return {"result": [{"spec": {
            "name": "sandbox_probe", "description": "Synthetic isolation fixture",
            "input_schema": {"type": "object"},
            "surface": {"kind": "function", "strict": False, "output_schema": None},
            "safety": "WritesFiles", "supports_parallel_tool_calls": False,
            "timeout_ms": 3000, "metadata": {},
        }, "model_visible": True, "user_command": None}]}
    if method != "invoke":
        raise ProtocolError("unexpected method")
    call = params["call"]
    allowed = Path("allowed.txt")
    allowed.write_text("workspace effect\n")
    checks = {"workspace_read_write": allowed.read_text() == "workspace effect\n",
              "explicit_environment": os.environ.get("SANDBOX_VISIBLE") == "yes"}
    for label, operation in (
        ("secret_denied", lambda: Path(call["args"]["secret"]).read_text()),
        ("symlink_denied", lambda: Path("secret-link").read_text()),
        ("outside_write_denied", lambda: Path(call["args"]["outside"]).write_text("escape")),
    ):
        try:
            operation()
        except OSError:
            checks[label] = True
        else:
            checks[label] = False
    return {"result": {"call_id": call["id"], "ok": True,
                       "output": json.dumps(checks), "content": [],
                       "error": None, "metadata": {}}}


run_component(initialize, invoke)

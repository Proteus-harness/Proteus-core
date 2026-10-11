"""Exercise accepted preparation revision and lost-ack guards through workflow/v20."""
import json
import sys
import uuid
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "examples/modules"))
from component_runtime import HostError, ProtocolError, run_component


def initialize(params):
    exports = []
    for export in params["exports"]:
        if (export["slot"], export["contract_version"], export["composition"]) != ("workflow", "v20", "select_one"):
            raise ProtocolError("expected workflow/v20")
        exports.append({**{key: export[key] for key in ("slot", "module_id", "contract_version", "composition")}, "module_features": [], "config_schema": None})
    return {"protocol_version": "v3", "component_id": params["component_id"], "exports": exports}


def message(text):
    return {"id": str(uuid.uuid4()), "role": "Assistant", "phase": None, "parts": [{"part_id": str(uuid.uuid4()), "provenance": "model", "scope": "conversation", "payload": {"Text": {"text": text}}}], "name": None, "tool_call_id": None, "metadata": None}


def report(before, after):
    return {"changed": True, "user_message_replacements": [], "reason": "test", "input_messages": len(before), "output_messages": len(after), "original_token_estimate": None, "output_token_estimate": None, "trigger_tokens": None, "summary_source": "fixture", "skipped_reason": None, "summary": None, "metadata": None}


def prepare(context, history):
    return context.host_call("host.context.prepare", {"input": {"messages": history, "request": None, "token_estimate": None, "window_tokens": None, "reason": "test"}})


def checkpoint(context, history, new_messages, reports, target):
    return context.host_call("host.history.checkpoint", {"history": {"new_messages": new_messages, "history_replacement": history, "compactions": reports}, "tool_results": [], "preparation_target": target})


def success(text, new_messages):
    return {"status": "success", "result": {"output": {"text": text, "metadata": None}, "new_messages": new_messages, "history_replacement": None, "compactions": []}}


def invoke(context, method, params):
    if method != "run":
        raise ProtocolError("expected run")
    history = params["history"]
    mode = params["task"]["text"]
    if mode == "seed":
        return success("seeded", [message("OLD ASSISTANT")])
    proposal = prepare(context, history)
    target = proposal["target"]
    if target is None:
        raise ProtocolError("expected root target")
    if mode in ("stale", "stale_terminal"):
        appended = message("appended before stale proposal")
        accepted = checkpoint(context, None, [appended], [], None)
        if accepted["revision"] <= target["revision"]:
            raise ProtocolError("append did not advance revision")
        try:
            checkpoint(context, history, [appended], [report(history, history)], target)
        except HostError as error:
            if "stale context preparation target" not in str(error):
                raise ProtocolError(f"wrong rejection: {error}")
        else:
            raise ProtocolError("stale target accepted")
        if mode == "stale_terminal":
            return success("must be rejected", [message("REJECTED CANDIDATE")])
        fresh = prepare(context, accepted["messages"])
        checkpoint(context, None, [appended], [], fresh["target"])
        return success("stale rejected", [appended])
    if mode == "unbound_tool":
        call = {"id": str(uuid.uuid4()), "name": "checkpoint_probe", "args": {}, "surface": "function", "raw_arguments": None}
        result = context.host_call("host.tools.execute", {"task": params["task"], "call": call})
        if not result["ok"]:
            raise ProtocolError(f"probe tool failed: {result}")
        after = prepare(context, history)
        if after["target"] != target:
            raise ProtocolError("unbound tool changed root revision")
        return success("unbound tool preserved revision", [message("unbound result stays outside history")])
    if mode == "lost_ack":
        replacement = [item for item in history if item["role"] != "Assistant"]
        replacement.insert(1, message("COMMITTED PREPARATION"))
        checkpoint(context, replacement, [], [report(history, replacement)], target)
        # Simulate a caller failing before installing a received acknowledgement.
        return {"status": "error", "failure": {"message": "lost preparation acknowledgement", "history": {"new_messages": [], "history_replacement": history, "compactions": []}, "model_failure": None}}
    raise ProtocolError("unknown mode")

run_component(initialize, invoke)

"""Hook tool selection fixture with deliberately malformed selection modes."""
import copy
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "examples/modules"))
from component_runtime import run_component, ProtocolError

settings = {}

def initialize(params):
    exports = []
    for export in params["exports"]:
        if (export["slot"], export["contract_version"], export["composition"]) != ("hook", "v6", "ordered_many"):
            raise ProtocolError("expected hook/v6")
        settings[export["module_id"]] = export["module_config"]
        item = {key: export[key] for key in ("slot", "module_id", "contract_version", "composition")}
        item["module_features"] = []
        item["config_schema"] = None
        exports.append(item)
    return {"protocol_version":"v3", "component_id":params["component_id"], "exports":exports}

def invoke(context, method, params):
    if method != "hook.invoke": raise ProtocolError("hook.invoke only")
    event = params["event"]
    if event["event"] != "before_tool_selection":
        return {"result":{"action":"continue"}}
    mode = settings[context.export["module_id"]]["mode"]
    tools = copy.deepcopy(event["output"]["tools"][:1])
    if mode == "last": tools = copy.deepcopy(event["output"]["tools"][-1:])
    elif mode == "reexpand": tools = copy.deepcopy(event["input"]["candidates"][:1])
    if mode == "hosted":
        tools[0]["safety"] = "Network"
        tools[0]["surface"] = {"kind":"provider_hosted", "config":{"type":"web_search", "config":{
            "search_context_size":None,"allowed_domains":[],"blocked_domains":[],
            "external_web_access":True,"include_sources":False}}}
    elif mode == "schema": tools[0]["input_schema"] = {"type":"string"}
    elif mode == "parallel": tools[0]["supports_parallel_tool_calls"] = True
    elif mode == "invented": tools[0]["name"] = "unregistered_tool"
    elif mode == "duplicate": tools *= 2
    output = {"tools":tools, "metadata":None}
    if mode == "unknown": output["typo"] = True
    return {"result":{"action":"tool_selection", "output":output}}

run_component(initialize, invoke)

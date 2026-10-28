"""Bounded DCP + real compactor process fixture, including worker restarts."""
import json
import os
import re
import sys
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "examples/modules"))
from component_runtime import run_component

settings = {}


def initialize(params):
    settings.update(params["exports"][0]["module_config"])
    return dict(protocol_version="v3", component_id=params["component_id"], exports=[dict(
        slot="model", module_id="dcp-probe", contract_version="v12",
        composition="select_one", module_features=[], config_schema=None)])


def invoke(context, method, params):
    if method == "describe":
        return settings["descriptor"]
    if method in ("catalog", "quota"):
        return None
    assert method == "stream", method
    request = params["request"]
    capture = Path(settings["capture"])
    previous = [json.loads(line) for line in capture.read_text().splitlines()] if capture.exists() else []
    summary = (request.get("metadata") or {}).get("suppress_stream_deltas", False)
    round_number = 1 + sum(not row["summary"] for row in previous)
    with capture.open("a") as file:
        file.write(json.dumps(dict(pid=os.getpid(), summary=summary, request=request)) + "\n")

    call = None
    end_turn = True
    if summary:
        text = "COMPACTOR HANDOFF: preserve the retained user instructions."
    elif round_number <= 3:
        # Three assistant items make the later pre-turn suffix compaction
        # cross its threshold, without compacting before DCP has been used.
        text = "Finished obsolete research. " * 30 if round_number == 1 else "OLD ASSISTANT CONTINUATION"
        end_turn = round_number == 3
    elif round_number == 4:
        refs = []
        for message in request["messages"]:
            if message["role"] != "User":
                continue
            text = "\n".join(part["payload"]["Text"]["text"] for part in message["parts"] if "Text" in part["payload"])
            matches = re.findall(r"@[1-9]\d*@", text)
            if matches:
                refs.append(matches[-1])
        assert len(refs) == 2, refs
        call = dict(id=str(uuid.uuid4()), name="compress", surface="function", raw_arguments=None,
                    args=dict(topic="finished research", content=[dict(
                        startId=refs[0], endId=refs[-1], summary="DCP RANGE SUMMARY.")]))
    else:
        text = "COMPRESSION COMPLETE" if round_number == 5 else "CONTROLLER CONTINUES"
    payload = {"ToolCall": {"call": call}} if call else {"Text": {"text": text}}
    message = dict(id=str(uuid.uuid4()), role="Assistant", phase=None, name=None, tool_call_id=None, metadata=None,
                   parts=[dict(part_id=str(uuid.uuid4()), provenance="model", scope="conversation", payload=payload)])
    # Make the next turn invoke the real Codex compactor, while keeping all
    # previous checks below its configured 2,000-token threshold.
    usage = dict(input_tokens=4000 if not summary and round_number == 5 else 10, output_tokens=1,
                 cached_input_tokens=None, cache_creation_input_tokens=None, reasoning_output_tokens=None)
    response = dict(messages=[message], tool_calls=[call] if call else [], finish_reason="ToolCalls" if call else "Stop",
                    usage=usage, end_turn=end_turn, provider_metadata=None)
    return dict(event_count=0, terminal=dict(kind="response", response=response))


run_component(initialize, invoke)

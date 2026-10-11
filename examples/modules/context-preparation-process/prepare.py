#!/usr/bin/env python3
"""Dependency-free context preparation hook for Proteus.

The module keeps a valid suffix beginning at one of the most recent user
turns. The hook/v6 boundary offers model and tool callbacks, but this
deterministic implementation does not need them.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from types import MappingProxyType
from collections.abc import Mapping
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from component_runtime import (  # noqa: E402
    PROTOCOL_VERSION,
    InvocationContext,
    ProtocolError,
    require_object,
    run_component,
)

SLOT = "hook"
MODULE_ID = "python_suffix"
CONTRACT_VERSION = "v6"

INITIALIZE_FIELDS = {
    "protocol_version",
    "component_id",
    "exports",
}
EXPORT_INITIALIZE_FIELDS = {
    "slot",
    "module_id",
    "contract_version",
    "composition",
    "module_config",
    "host_features",
}
INPUT_FIELDS = {
    "messages",
    "request",
    "token_estimate",
    "window_tokens",
    "reason",
}
STRATEGY_FIELDS = {"trigger_messages", "retain_user_turns"}
strategy_config: Mapping[str, int] | None = None


def positive_int(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise ProtocolError(f"{label} must be a positive integer")
    return value


def validate_strategy(strategy: Any) -> tuple[int, int]:
    if not isinstance(strategy, dict):
        raise ProtocolError("compactor strategy must be an object")
    unknown = sorted(set(strategy) - STRATEGY_FIELDS)
    if unknown:
        raise ProtocolError(f"unknown compactor strategy fields: {unknown}")
    trigger_messages = positive_int(strategy.get("trigger_messages", 12), "trigger_messages")
    retain_user_turns = positive_int(
        strategy.get("retain_user_turns", 2), "retain_user_turns"
    )
    return trigger_messages, retain_user_turns


def validate_initialize(params: Any) -> str:
    params = require_object(params, INITIALIZE_FIELDS, "initialize params")
    if params["protocol_version"] != PROTOCOL_VERSION:
        raise ProtocolError(f"unsupported component protocol: {params['protocol_version']!r}")
    component_id = params["component_id"]
    if not isinstance(component_id, str) or not component_id.strip():
        raise ProtocolError("initialize component_id must be a non-empty string")
    exports = params["exports"]
    if not isinstance(exports, list) or len(exports) != 1:
        raise ProtocolError("python preparation component requires exactly one export")
    export = require_object(
        exports[0], EXPORT_INITIALIZE_FIELDS, "initialize export"
    )
    expected_identity = (
        SLOT,
        MODULE_ID,
        CONTRACT_VERSION,
        "ordered_many",
    )
    actual_identity = (
        export["slot"],
        export["module_id"],
        export["contract_version"],
        export["composition"],
    )
    if actual_identity != expected_identity:
        raise ProtocolError(f"unsupported initialize export: {export!r}")
    if not isinstance(export["module_config"], dict):
        raise ProtocolError("initialize module_config must be an object")
    if export["host_features"] != []:
        raise ProtocolError("hook v6 does not negotiate host features")
    return component_id


def validate_input(params: Any, strategy: Any) -> tuple[dict[str, Any], int, int]:
    preparation = require_object(params, INPUT_FIELDS, "ContextPreparationInput")
    if preparation["request"] is not None and not isinstance(preparation["request"], dict):
        raise ProtocolError("ContextPreparationInput.request must be an object or null")
    messages = preparation["messages"]
    if not isinstance(messages, list) or any(not isinstance(message, dict) for message in messages):
        raise ProtocolError("ContextPreparationInput.messages must contain canonical objects")
    for message in messages:
        parts = message.get("parts")
        if not isinstance(parts, list):
            raise ProtocolError("canonical message parts must be an array")
        if any(
            not isinstance(part, dict)
            or part.get("scope") not in ("conversation", "request", "trace")
            for part in parts
        ):
            raise ProtocolError("canonical part scope must be conversation, request or trace")
    trigger_messages, retain_user_turns = validate_strategy(strategy)
    return preparation, trigger_messages, retain_user_turns


def is_context_message(message: dict[str, Any]) -> bool:
    # Match the common canonical scope contract, not a workflow's choice of
    # message name. Empty messages are not request-only context.
    parts = message["parts"]
    return bool(parts) and all(part["scope"] == "request" for part in parts)


def is_user_message(message: dict[str, Any]) -> bool:
    return message.get("role") == "User" and not is_context_message(message)


def unchanged(messages: list[dict[str, Any]], reason: str) -> dict[str, Any]:
    return {
        "output": {
            "messages": messages,
            "user_message_replacements": [],
            "changed": False,
            "summary": None,
            "token_estimate": None,
            "original_token_estimate": None,
            "trigger_tokens": None,
            "summary_source": None,
            "skipped_reason": reason,
            "metadata": {
                "strategy": MODULE_ID,
            },
        }
    }


def compact(params: Any, strategy: Any) -> dict[str, Any]:
    compaction, trigger_messages, retain_user_turns = validate_input(params, strategy)
    messages = compaction["messages"]
    if len(messages) <= trigger_messages:
        return unchanged(messages, "below_trigger_messages")

    user_indices = [
        index for index, message in enumerate(messages) if is_user_message(message)
    ]
    if not user_indices:
        return unchanged(messages, "no_user_turn_boundary")

    retained_user_count = min(retain_user_turns, len(user_indices))
    suffix_start = user_indices[-retained_user_count]
    context_prefix = [
        message
        for index, message in enumerate(messages)
        if index < suffix_start and is_context_message(message)
    ]
    retained = context_prefix + messages[suffix_start:]
    if len(retained) >= len(messages):
        return unchanged(messages, "suffix_would_not_reduce_history")

    retained_users = sum(1 for message in retained if is_user_message(message))
    summary = (
        f"Deterministic suffix compaction retained {retained_users} recent user "
        f"turns and dropped {len(messages) - len(retained)} earlier messages."
    )
    return {
        "output": {
            "messages": retained,
            "user_message_replacements": [],
            "changed": True,
            "summary": summary,
            "token_estimate": None,
            "original_token_estimate": None,
            "trigger_tokens": None,
            "summary_source": "deterministic_suffix",
            "skipped_reason": None,
            "metadata": {
                "strategy": MODULE_ID,
                "retained_user_turns": retained_users,
            },
        }
    }


def initialize(params: Any) -> dict[str, Any]:
    global strategy_config
    component_id = validate_initialize(params)
    trigger_messages, retain_user_turns = validate_strategy(params["exports"][0]["module_config"])
    strategy_config = MappingProxyType({
        "trigger_messages": trigger_messages,
        "retain_user_turns": retain_user_turns,
    })
    return {
        "protocol_version": PROTOCOL_VERSION,
        "component_id": component_id,
        "exports": [
            {
                "slot": SLOT,
                "module_id": MODULE_ID,
                "contract_version": CONTRACT_VERSION,
                "composition": "ordered_many",
                "module_features": [], "config_schema": None,
            }
        ],
    }


def invoke(context: InvocationContext, method: str, params: Any) -> dict[str, Any]:
    if context.export != {"slot": SLOT, "module_id": MODULE_ID}:
        raise ProtocolError(f"unknown component export: {context.export!r}")
    if method != "hook.invoke":
        raise ProtocolError(f"unknown method: {method!r}")
    if strategy_config is None:
        raise ProtocolError("python preparation export is not initialized")
    context.ensure_active()
    hook = require_object(params, {"event", "attribution", "cwd", "conversation"}, "HookInput")
    event = hook["event"]
    if not isinstance(event, dict):
        raise ProtocolError("hook event must be an object")
    if event.get("event") != "before_context":
        return {"result": {"action": "continue"}}
    event = require_object(event, {"event", "input", "output", "target"}, "BeforeContext")
    prior = event["output"]
    if not isinstance(prior, dict) or not isinstance(prior.get("messages"), list):
        raise ProtocolError("BeforeContext output must be a preparation candidate")
    preparation = require_object(event["input"], INPUT_FIELDS, "ContextPreparationInput")
    validate_input(preparation, dict(strategy_config))
    result = compact({**preparation, "messages": prior["messages"]}, dict(strategy_config))["output"]
    context.ensure_active()
    if prior.get("changed") and not result["changed"]:
        return {"result": {"action": "continue"}}
    return {"result": {"action": "context_preparation", "output": result}}


def main() -> int:
    run_component(initialize, invoke)


if __name__ == "__main__":
    raise SystemExit(main())

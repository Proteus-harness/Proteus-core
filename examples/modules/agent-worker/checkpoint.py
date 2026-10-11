"""Adopt accepted workflow history without persisting request-only context."""
from __future__ import annotations
from typing import Any
from component_runtime import ProtocolError, require_object


def accept_checkpoint(
    acknowledgement: dict[str, Any],
    model_messages: list[dict[str, Any]],
    previous: dict[str, Any],
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    history = require_object(acknowledgement["history"],
        {"new_messages", "history_replacement", "compactions"}, "accepted history")
    messages = acknowledgement["messages"]
    revision = acknowledgement["revision"]
    if not isinstance(messages, list) or any(not isinstance(message, dict) for message in messages):
        raise ProtocolError("checkpoint accepted messages must be an array of canonical objects")
    if not isinstance(history["new_messages"], list) or not isinstance(history["compactions"], list):
        raise ProtocolError("checkpoint accepted history has invalid arrays")
    if history["history_replacement"] is not None and not isinstance(history["history_replacement"], list):
        raise ProtocolError("checkpoint accepted replacement must be an array or null")
    if revision is None:
        return previous, model_messages
    if not isinstance(revision, int) or isinstance(revision, bool) or revision < 0:
        raise ProtocolError("checkpoint revision must be a nonnegative integer or null")
    # This example builds request context once, as a prefix. Canonical ACK
    # replaces only its conversation projection, including host-added results.
    context = [message for message in model_messages
        if message.get("parts") and all(part.get("scope") == "request" for part in message["parts"])]
    return history, context + messages

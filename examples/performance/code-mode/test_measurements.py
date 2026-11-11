"""Usage completeness and nested ownership must not exaggerate CodeMode savings."""
from pathlib import Path
import unittest
from unittest.mock import patch

from measurements import measure


def request(identity):
    return ({"kind": "model_request_recorded"}, {
        "exchange_id": identity, "origin": "direct",
        "request": {"model": {"provider": "openai_codex", "model": "fixture"},
                    "reasoning": {"effort": "xhigh", "summary": True}, "messages": [], "tools": []}})


def response(identity, usage):
    return ({"kind": "model_response_recorded"}, {
        "exchange_id": identity, "outcome": {"status": "response", "response": {"usage": usage}}})


class MeasurementTests(unittest.TestCase):
    def measured(self, records):
        with patch("measurements._journal.records", return_value=iter(records)):
            return measure(Path("synthetic-session"))

    def test_partial_usage_cannot_be_presented_as_a_total(self):
        m = self.measured([request("a"), response("a", {"input_tokens": 120, "output_tokens": 10}),
                           request("b"), response("b", None)])
        self.assertIsNone(m["provider_usage"]["input_tokens"])
        self.assertIsNone(m["provider_usage"]["output_tokens"])
        self.assertEqual(m["partial_reported_usage_sums"]["input_tokens"], 120)
        self.assertEqual(m["usage_fields_reported_by_responses"]["input_tokens"], 1)

    def test_zero_cached_tokens_are_reported_not_missing(self):
        m = self.measured([request("a"), response("a", {
            "input_tokens": 120, "output_tokens": 10, "cached_input_tokens": 0,
            "cache_creation_input_tokens": 0, "reasoning_output_tokens": 0})])
        self.assertEqual(m["provider_usage"]["cached_input_tokens"], 0)
        self.assertEqual(m["provider_usage"]["uncached_input_tokens"], 120)
        self.assertEqual(m["provider_usage"]["reasoning_output_tokens"], 0)

    def test_missing_response_does_not_undercount_the_completed_request_usage(self):
        m = self.measured([request("a"), response("a", {"input_tokens": 100, "output_tokens": 5}),
                           request("b")])
        self.assertEqual(m["model_requests"], 2)
        self.assertEqual(m["model_responses"], 1)
        self.assertIsNone(m["provider_usage"]["input_tokens"])

    def test_root_script_and_nested_api_are_distinct_operations(self):
        def call(identity, name, owner):
            return ({"kind": "tool_call_recorded"}, {
                "call": {"id": identity, "name": name},
                "phase": {"phase": "requested", "owner": {"owner": owner}}})
        m = self.measured([call("script", "code_mode", "workflow"),
                           call("nested", "lab__get_issue", "tool"),
                           call("probe", "lab__list_issues", "workflow")])
        self.assertEqual(m["all_tool_calls"], 3)
        self.assertEqual(m["model_visible_tool_calls"], 2)
        self.assertEqual(m["root_mcp_tool_calls"], 1)
        self.assertEqual(m["nested_mcp_tool_calls"], 1)


if __name__ == "__main__":
    unittest.main()

"""The report must describe completed work, even when its guessed contents match."""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest

from fixtures import create_case, read_json, score_case, write_json


class SubmissionOrderTests(unittest.TestCase):
    def scored_audit(self, submit_first):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory)
            task = create_case("audit", state)
            issues = read_json(state / "initial.json")["issues"]
            events = []

            def event(method, params, result):
                events.append({"ordinal": len(events) + 1, "method": method,
                               "params": params, "outcome": {"ok": True, "result": result}})

            def submit():
                event("submit_report", {"report": deepcopy(task["expected_report"])}, {"accepted": True})
                write_json(state / "report.json", task["expected_report"])

            if submit_first:
                submit()
            for offset in range(0, len(issues), 8):
                event("list_issues", {"cursor": None}, {
                    "issues": [{"id": issue["id"]} for issue in issues[offset:offset + 8]],
                    "next_cursor": None})
            for issue in issues:
                event("get_issue", {"id": issue["id"]}, issue)
            if not submit_first:
                submit()
            (state / "api.jsonl").write_text("".join(json.dumps(row) + "\n" for row in events))
            return score_case("audit", state)

    def test_correct_report_before_work_is_not_completion(self):
        score = self.scored_audit(submit_first=True)
        self.assertTrue(score["exact_checks"]["report_exact"])
        self.assertTrue(score["exact_checks"]["single_submission"])
        self.assertEqual(score["issues"], ["submission_after_all_operations"])
        self.assertFalse(score["passed"])

    def test_report_after_complete_work_passes(self):
        self.assertTrue(self.scored_audit(submit_first=False)["passed"])


if __name__ == "__main__":
    unittest.main()

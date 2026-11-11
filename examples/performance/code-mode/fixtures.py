"""Deterministic synthetic issue workloads and an independent result oracle."""
from collections import Counter
from copy import deepcopy
import hashlib
import json
from pathlib import Path


SEED = "proteus-codemode-api-v1"
PROMPT_VERSION = "v2-explicit-count-and-single-update"
TEAMS = ("core", "desktop", "integrations")


def write_json(path, value):
    path = Path(path)
    pending = path.with_suffix(path.suffix + ".pending")
    pending.write_text(json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n")
    pending.replace(path)


def read_json(path):
    return json.loads(Path(path).read_text())


def _issues(case):
    result = []
    for index in range(64 if case == "audit" else 48):
        digest = hashlib.sha256(f"{SEED}:{case}:{index}".encode()).hexdigest()[:12]
        description = (
            f"Synthetic issue {digest}. A regression report describes interaction between "
            f"component {index % 7} and request group {index % 13}. The reporter observed "
            "intermittent retries while validating a local development build. Logs include "
            "several successful requests and one incomplete trace. Reproduce with the "
            "documented fixture before changing any implementation. Keep the original "
            "acceptance criteria, labels, owner and estimate unchanged during queue cleanup. "
        )
        issue = {
            "id": f"issue_{digest}",
            "title": f"Synthetic component regression {index + 1:03d}",
            "team": TEAMS[index % len(TEAMS)],
            "status": "closed" if index % 5 == 0 else "open",
            "priority": ("p0", "p1", "p2", "p3")[index % 4],
            "last_activity_days": (index * 7 + 3) % 35,
            "blocked": index % 11 == 0,
            "estimate_minutes": ((index * 13) % 17 + 1) * 15,
            "queue": "Triage",
            "approved_for_backlog": index % 3 == 0,
            "owner": f"synthetic-owner-{index % 9}",
            "labels": ["synthetic", f"component-{index % 7}"],
            "custom": {"batch": index // 8, "checksum": digest, "reviewed": index % 2 == 0},
            "description": (description * 3)[:900],
        }
        if case == "update":
            # Each group has one matching record and two different exclusions.
            issue["queue"] = "InProgress" if index % 3 == 2 else "Triage"
            issue["approved_for_backlog"] = index % 3 != 1
            issue["status"] = "closed" if index % 6 == 2 else "open"
        result.append(issue)
    return result


def _targets(case, issues):
    if case == "audit":
        return [issue for issue in issues if issue["status"] == "open"
                and issue["priority"] in ("p0", "p1")
                and issue["last_activity_days"] >= 14 and not issue["blocked"]]
    return [issue for issue in issues if issue["queue"] == "Triage"
            and issue["approved_for_backlog"] is True and issue["status"] == "open"]


def _expected_report(case, issues):
    selected = _targets(case, issues)
    ids = sorted(issue["id"] for issue in selected)
    if case == "audit":
        counts = Counter(issue["team"] for issue in selected)
        return {"examined": len(issues), "matched_ids": ids,
                "total_estimate_minutes": sum(issue["estimate_minutes"] for issue in selected),
                "by_team": {team: counts[team] for team in TEAMS}}
    return {"examined": len(issues), "updated_ids": ids, "verified_ids": ids, "failures": []}


def create_case(case: str, state_dir: Path) -> dict:
    if case not in ("audit", "update"):
        raise ValueError(f"unknown benchmark case: {case}")
    state_dir = Path(state_dir)
    state_dir.mkdir(parents=True, exist_ok=True)
    if any((state_dir / name).exists() for name in ("initial.json", "current.json", "report.json", "api.jsonl")):
        raise ValueError(f"refusing to overwrite an existing benchmark state: {state_dir}")
    issues = _issues(case)
    state = {"case": case, "seed": SEED, "issues": issues}
    write_json(state_dir / "initial.json", state)
    write_json(state_dir / "current.json", state)
    (state_dir / "api.jsonl").write_text("")
    expected = _expected_report(case, issues)
    target_count = len(_targets(case, issues))
    assert target_count >= 8 if case == "audit" else target_count == 16
    common = (
        "Use the lab issue API. Call lab__list_issues with cursor=null and follow every "
        "next_cursor until it is null; each page contains only IDs, at most 8. Fetch the "
        "full details of every listed issue with lab__get_issue. Do not infer unseen records "
        "or skip pages. Complete the task using the available API tools, then call "
        "lab__submit_report exactly once with the report object described below. "
        "The tools return structured JSON; a delegated ToolResult.output contains its JSON text. "
    )
    if case == "audit":
        prompt = common + (
            "Audit all 64 issues without changing any issue. Match issues whose status is "
            "'open', priority is 'p0' or 'p1', last_activity_days is at least 14, and blocked "
            "is false. Submit exactly these report fields: examined (number of all examined "
            "issues), matched_ids (matching IDs sorted lexicographically), "
            "total_estimate_minutes (sum of estimate_minutes of matches), by_team (matching "
            "counts for core, desktop, integrations, including zeros). Give only a short "
            "completion summary after submission."
        )
    else:
        prompt = common + (
            "Inspect all 48 issues. For exactly those with queue='Triage', "
            "approved_for_backlog=true and status='open', call lab__update_queue to move "
            "them to queue='Backlog', exactly one update per matching ID; do not repeat "
            "updates. Change no other issue or field. Keep each original "
            "record, and after every update fetch that issue again with lab__get_issue "
            "and verify that only queue changed and the new queue is Backlog. Submit "
            "exactly these report fields: examined (integer count of all examined issues, "
            "not the issue records), updated_ids "
            "(successfully updated IDs sorted lexicographically), verified_ids "
            "(successfully read-back and verified IDs in the same sorted order), failures "
            "(an empty array if all checks succeeded, otherwise describe the failures). "
            "Give only a short completion summary after submission."
        )
    return {"case": case, "prompt": prompt, "seed": SEED, "issue_count": len(issues),
            "prompt_version": PROMPT_VERSION,
            "prompt_sha256": hashlib.sha256(prompt.encode()).hexdigest(),
            "target_count": target_count, "expected_report": expected,
            "fixture_sha256": hashlib.sha256((state_dir / "initial.json").read_bytes()).hexdigest()}


def score_case(case: str, state_dir: Path) -> dict:
    state_dir = Path(state_dir)
    initial = read_json(state_dir / "initial.json")
    if initial["case"] != case:
        raise ValueError("case does not match the immutable fixture")
    current = read_json(state_dir / "current.json")
    report_path = state_dir / "report.json"
    submitted = read_json(report_path) if report_path.exists() else None
    events = [json.loads(line) for line in (state_dir / "api.jsonl").read_text().splitlines() if line.strip()]
    successful = [event for event in events if event["outcome"]["ok"]]
    issues = initial["issues"]
    all_ids = {issue["id"] for issue in issues}
    targets = {issue["id"] for issue in _targets(case, issues)}
    gets = [event for event in successful if event["method"] == "get_issue"]
    updates = [event for event in events if event["method"] == "update_queue"]
    submissions = [event for event in events if event["method"] == "submit_report"]
    listed = {issue["id"] for event in successful if event["method"] == "list_issues"
              for issue in event["outcome"]["result"]["issues"]}
    expected_state = deepcopy(initial)
    if case == "update":
        for issue in expected_state["issues"]:
            if issue["id"] in targets:
                issue["queue"] = "Backlog"
    expected_ids = targets if case == "update" else set()
    actual_updates = [event["params"].get("id") for event in updates]
    checks = {
        "report_exact": submitted == _expected_report(case, issues),
        "all_pages_listed": listed == all_ids,
        "all_records_read": {event["params"].get("id") for event in gets} == all_ids,
        "full_state_exact": current == expected_state,
        "update_scope_exact": set(actual_updates) == expected_ids,
        "no_duplicate_updates": len(actual_updates) == len(set(actual_updates)),
        "single_submission": len(submissions) == 1 and submissions[0]["outcome"]["ok"],
        "submission_after_all_operations": len(submissions) == 1 and all(
            event["ordinal"] < submissions[0]["ordinal"]
            for event in events if event["method"] != "submit_report"
        ),
    }
    if case == "update":
        # Log ordinals are assigned while holding the state lock. A later get
        # therefore observes the committed update, not merely request admission.
        originals = {issue["id"]: issue for issue in issues}
        final = {issue["id"]: issue for issue in expected_state["issues"]}
        checks["every_update_read_before_and_after"] = all(
            any(read["ordinal"] < update["ordinal"] and read["params"].get("id") == update["params"].get("id")
                and read["outcome"]["result"] == originals.get(update["params"].get("id")) for read in gets)
            and any(read["ordinal"] > update["ordinal"] and read["params"].get("id") == update["params"].get("id")
                    and read["outcome"]["result"] == final.get(update["params"].get("id")) for read in gets)
            for update in updates
        ) and len(updates) == len(targets)
    return {"passed": all(checks.values()), "exact_checks": checks,
            "issues": [name for name, passed in checks.items() if not passed],
            "expected_report": _expected_report(case, issues), "submitted_report": submitted,
            "api_calls": len(events), "api_counts": dict(Counter(event["method"] for event in events)),
            "failed_api_calls": len(events) - len(successful),
            "distinct_records_read": len({event["params"].get("id") for event in gets}),
            "expected_updates": len(expected_ids), "attempted_updates": len(updates)}

"""Optional declared API contracts; no fixture IDs, totals or selection oracle."""
from copy import deepcopy


def obj(properties):
    return {"type": "object", "properties": properties,
            "required": list(properties), "additionalProperties": False}


STRING = {"type": "string"}
INTEGER = {"type": "integer"}
IDS = {"type": "array", "items": STRING, "uniqueItems": True}
ISSUE = obj({
    **{name: STRING for name in ("id", "title", "team", "status", "priority", "queue", "owner", "description")},
    **{name: INTEGER for name in ("last_activity_days", "estimate_minutes")},
    "blocked": {"type": "boolean"}, "approved_for_backlog": {"type": "boolean"},
    "labels": {"type": "array", "items": STRING},
    "custom": obj({"batch": INTEGER, "checksum": STRING, "reviewed": {"type": "boolean"}}),
})
OUTPUTS = {
    "list_issues": obj({
        "issues": {"type": "array", "maxItems": 8, "items": obj({"id": STRING})},
        "next_cursor": {"type": ["string", "null"]},
    }),
    "get_issue": ISSUE,
    "update_queue": obj({"id": STRING, "queue": STRING}),
    "submit_report": obj({"stored": {"type": "boolean"}}),
}


def declared_tools(tools, case):
    """Add only the service's known field/type contract, not expected answers."""
    result = deepcopy(tools)
    report = obj({"examined": {"type": "integer", "minimum": 0}, **({
        "matched_ids": IDS, "total_estimate_minutes": INTEGER,
        "by_team": obj({team: INTEGER for team in ("core", "desktop", "integrations")}),
    } if case == "audit" else {
        "updated_ids": IDS, "verified_ids": IDS, "failures": {"type": "array", "items": {}},
    })})
    for tool in result:
        tool["outputSchema"] = deepcopy(OUTPUTS[tool["name"]])
        if tool["name"] == "submit_report":
            tool["inputSchema"]["properties"]["report"] = report
    return result

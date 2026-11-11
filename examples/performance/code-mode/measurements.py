"""Read provider usage and operation ownership from the canonical journal."""
import collections
import hashlib
import importlib.util
import json
from pathlib import Path


_spec = importlib.util.spec_from_file_location(
    "performance_journal", Path(__file__).parents[1] / "history-scale/journal.py")
_journal = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_journal)

USAGE_FIELDS = ("input_tokens", "output_tokens", "cached_input_tokens",
                "cache_creation_input_tokens", "reasoning_output_tokens")


def measure(session):
    requests, responses, calls = {}, {}, {}
    spec_hashes = collections.defaultdict(set)
    tools_by_owner = collections.Counter()
    tool_names = collections.Counter()
    failures, settlements, compactions = [], [], 0
    opened_at, settled_at = None, None
    model_tool_result_bytes = 0
    for record, payload in _journal.records(session):
        kind = record["kind"]
        if kind == "turn_opened":
            opened_at = record["timestamp_ms"]
        elif kind == "model_request_recorded":
            identity = payload["exchange_id"]
            if identity in requests:
                raise ValueError("duplicate canonical model exchange")
            request = payload["request"]
            for spec in request["tools"]:
                if spec["name"] == "code_mode" or spec["name"].startswith("lab__"):
                    encoded = json.dumps(spec, sort_keys=True, ensure_ascii=False).encode()
                    spec_hashes[spec["name"]].add(hashlib.sha256(encoded).hexdigest())
            result_bytes = 0
            for message in request["messages"]:
                for part in message["parts"]:
                    result = part["payload"].get("ToolResult")
                    if result:
                        result_bytes += len(json.dumps(result, ensure_ascii=False).encode())
            model_tool_result_bytes += result_bytes
            requests[identity] = {"origin": payload["origin"], "model": request["model"],
                                  "reasoning": request.get("reasoning"),
                                  "tool_result_bytes": result_bytes}
        elif kind == "model_response_recorded":
            identity = payload["exchange_id"]
            if identity not in requests or identity in responses:
                raise ValueError("canonical model response ownership mismatch")
            outcome = payload["outcome"]
            responses[identity] = outcome
            if outcome["status"] != "response":
                failures.append({"kind": "model", "exchange_id": identity,
                                 "failure": outcome.get("failure")})
        elif kind == "tool_call_recorded" and payload["phase"]["phase"] == "requested":
            call = payload["call"]
            if call["id"] in calls:
                raise ValueError("duplicate canonical tool call")
            owner = payload["phase"]["owner"]["owner"]
            calls[call["id"]] = {"name": call["name"], "owner": owner}
            tools_by_owner[owner] += 1
            tool_names[call["name"]] += 1
        elif kind == "tool_result_recorded" and not payload["result"]["ok"]:
            result = payload["result"]
            failures.append({"kind": "tool", "call_id": result["call_id"],
                             "name": calls.get(result["call_id"], {}).get("name"),
                             "error": result.get("error")})
        elif kind == "turn_settled":
            settled_at = record["timestamp_ms"]
            settlements.append({"status": payload["status"], "error": payload.get("error")})
        elif kind == "history_mutated" and payload.get("compaction"):
            compactions += 1

    usage_rows = [outcome["response"].get("usage") for outcome in responses.values()
                  if outcome["status"] == "response"]
    usage, reported_sums = {}, {}
    known = {}
    for field in USAGE_FIELDS:
        values = [row[field] for row in usage_rows if row is not None and row.get(field) is not None]
        known[field] = len(values)
        reported_sums[field] = sum(values) if values else None
        usage[field] = sum(values) if values and len(values) == len(requests) else None
    input_tokens = usage["input_tokens"]
    cached = usage["cached_input_tokens"]
    usage["uncached_input_tokens"] = (
        input_tokens - cached if input_tokens is not None and cached is not None
        and known["cached_input_tokens"] == len(usage_rows) else None)
    return {"session": str(Path(session).resolve()), "model_requests": len(requests),
            "model_responses": len(responses), "provider_usage": usage,
            "partial_reported_usage_sums": reported_sums,
            "usage_fields_reported_by_responses": known,
            "model_refs": sorted({json.dumps(row["model"], sort_keys=True) for row in requests.values()}),
            "reasoning_refs": sorted({json.dumps(row["reasoning"], sort_keys=True) for row in requests.values()}),
            "tool_spec_sha256": {name: sorted(hashes) for name, hashes in spec_hashes.items()},
            "model_visible_tool_calls": tools_by_owner["workflow"],
            "nested_tool_calls": tools_by_owner["tool"], "all_tool_calls": len(calls),
            "root_mcp_tool_calls": sum(call["owner"] == "workflow" and call["name"].startswith("lab__")
                                       for call in calls.values()),
            "nested_mcp_tool_calls": sum(call["owner"] == "tool" and call["name"].startswith("lab__")
                                         for call in calls.values()),
            "tool_names": dict(tool_names), "failures": failures, "compactions": compactions,
            "settlements": settlements, "model_tool_result_bytes_across_requests": model_tool_result_bytes,
            "turn_wall_seconds": ((settled_at - opened_at) / 1000
                                  if opened_at is not None and settled_at is not None else None),
            "requests": [{"exchange_id": key, **row} for key, row in requests.items()]}


def write_markdown(report, destination):
    rows = ["# CodeMode: живое сравнение API orchestration", "",
            f"Модель: `{report['model']}`, reasoning `{report['reasoning']}`. "
            f"Core: `{report['revisions']['core']}`; Modules: `{report['revisions']['modules']}`.", "",
            f"Объявленные output/report schemas API: {'включены' if report['api_schemas'] else 'выключены'}.", "",
            "Синтетический stdio MCP: отбор 64 карточек и перенос 16 из 48 с readback. "
            "Прямому режиму разрешены пакетные вызовы. Между режимами меняются только "
            "доступность CodeMode и инструкция способа вызова; общие tools и данные одинаковы.", "",
            "Время включает запуск CLI и завершение процессов. Токены взяты из provider usage, "
            "cached входят во входные; reasoning входит в выходные. Денежная стоимость не вычисляется.", "",
            "| Задача | Режим | Задача | CLI | Секунды | Model requests | Input | Cached input | Output | Reasoning | Root tools | API calls |",
            "|---|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for run in report["runs"]:
        metrics = run.get("metrics", {})
        usage = metrics.get("provider_usage", {})
        value = lambda key: usage.get(key) if usage.get(key) is not None else "n/a"
        status = "PASS" if run.get("task_passed", run["passed"]) else "FAIL"
        cli = "OK" if run["returncode"] == 0 else f"exit {run['returncode']}"
        rows.append(f"| {run['case']} | {run['mode']} | {status} | {cli} | {run['wall_seconds']:.2f} | "
                    f"{metrics.get('model_requests', 'n/a')} | {value('input_tokens')} | "
                    f"{value('cached_input_tokens')} | {value('output_tokens')} | "
                    f"{value('reasoning_output_tokens')} | {metrics.get('model_visible_tool_calls', 'n/a')} | "
                    f"{run['score'].get('api_calls', 'n/a')} |")
    rows += ["", "API calls — реальные обращения к сервису; один code_mode не считается одним API call. "
             "Неуспешные попытки сохранены. Новые сессии, prompt cache включён; порядок режимов меняется. "
             "Один прогон на режим и задачу — диагностическое наблюдение, а не оценка всех coding workloads.", ""]
    for run in report["runs"]:
        if not run["passed"]:
            rows += [f"Ошибка {run['id']}: `{run.get('error') or run['score'].get('issues')}`", ""]
    destination.write_text("\n".join(rows))

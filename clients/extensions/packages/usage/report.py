"""Usage calculations moved out of the client; pinned tariff table retained."""

RATES = {
    "gpt-6-astra": (10, 1, 12.5, 50),
    "gpt-5.6-sol": (4, .4, 5, 20),
    "gpt-5.6": (4, .4, 5, 20),
    "gpt-5.6-terra": (2, .2, 2.5, 12),
    "gpt-5.6-luna": (.2, .02, .25, 1.2),
}
PRICE_DATE = "2026-09-10"
PRICE_SOURCE = "https://developers.openai.com/api/docs/pricing"


def cost(request):
    usage = request.get("usage")
    rate = RATES.get(request["model"]["model"])
    if usage is None or rate is None:
        return None
    input_tokens = usage["input_tokens"]
    cached = usage.get("cached_input_tokens") or 0
    written = usage.get("cache_creation_input_tokens") or 0
    if cached + written > input_tokens:
        return None
    fresh = input_tokens - cached - written
    long = input_tokens > 272000
    return ((fresh * rate[0] + cached * rate[1] + written * rate[2]) * (2 if long else 1)
            + usage["output_tokens"] * rate[3] * (1.5 if long else 1)) / 1000000


def model_key(request):
    model = request["model"]
    return model["provider"] + "/" + model["model"]


def duration(request):
    end = request.get("finished_at_ms")
    return max(0, end - request["started_at_ms"]) if end is not None else 0


def scoped(snapshot, period, model):
    requests = snapshot["requests"]
    if period != "all":
        turn = snapshot.get("latest_turn_id") if period == "latest" else period
        requests = [request for request in requests if request.get("turn_id") == turn]
    if model is not None:
        requests = [request for request in requests if model_key(request) == model]
    return requests


def filtered(requests, status, query, order):
    rows = [request for request in requests
            if (status != "problems" or request["status"] != "completed")
            and (status != "missing" or request.get("usage") is None)
            and query.lower() in (model_key(request) + " " + request["exchange_id"]
                                  + " " + str(request.get("turn_id") or "")).lower()]
    keys = {
        "oldest": lambda request: request["started_at_ms"],
        "newest": lambda request: -request["started_at_ms"],
        "longest": lambda request: -duration(request),
        "tokens": lambda request: -sum((request.get("usage") or {}).get(field, 0) for field in ("input_tokens","output_tokens")),
        "cost": lambda request: -(cost(request) or 0),
    }
    return sorted(rows, key=keys[order])


def totals(requests):
    usages = [request["usage"] for request in requests if request.get("usage") is not None]
    prices = [value for request in requests if (value := cost(request)) is not None]
    result = {field:sum(usage.get(field) or 0 for usage in usages) for field in (
        "input_tokens","output_tokens","cached_input_tokens","cache_creation_input_tokens","reasoning_output_tokens")}
    for field in ("cached_input_tokens","cache_creation_input_tokens","reasoning_output_tokens"):
        if not any(usage.get(field) is not None for usage in usages): result[field]=None
    result.update(requests=len(requests), reported=len(usages), cost=sum(prices), priced=len(prices),
                  auxiliary=sum(request["origin"] == "auxiliary" for request in requests),
                  unfinished=sum(request["status"] == "unfinished" for request in requests),
                  errors=sum(request["status"] in ("error","canceled","timeout") for request in requests))
    return result


def context(snapshot):
    usage = snapshot.get("latest_usage") or {}
    actual = usage.get("actual")
    used = actual["input_tokens"] if actual else usage.get("estimated_input_tokens", snapshot["history"]["estimated_tokens"])
    maximum = usage.get("max_input_tokens")
    cached = (actual or {}).get("cached_input_tokens")
    return used, maximum, cached, usage


def cells(snapshot):
    used, maximum, _, usage = context(snapshot)
    maximum=maximum or max(used,1)
    categories=usage.get("categories",[])
    total=sum(category["tokens"] for category in categories)
    tones=["accent","success","warning","default","muted"]
    segments=[(category["name"],category["tokens"] / total * used,tones[index%len(tones)]) for index,category in enumerate(categories) if total and category["tokens"]]
    if not segments and used:segments=[("Неразмеченный контекст",used,"accent")]
    trigger=usage.get("compaction_trigger_tokens") or maximum
    if max(used,trigger)<maximum: segments.append(("Резерв",maximum-max(used,trigger),"danger"))
    if used<min(maximum,trigger): segments.append(("Свободно",min(maximum,trigger)-used,"muted"))
    total=sum(value for _,value,_ in segments) or 1
    counts=[int(value/total*200) for _,value,_ in segments]
    for index in sorted(range(len(segments)),key=lambda index: segments[index][1]/total*200-counts[index],reverse=True)[:200-sum(counts)]: counts[index]+=1
    # Preserve small nonzero categories, including compaction reserve.
    for index, count in enumerate(counts):
        if count == 0 and segments[index][1] > 0:
            donor = max(range(len(counts)), key=counts.__getitem__)
            if counts[donor] > 1:
                counts[donor] -= 1
                counts[index] = 1
    return [{"label":f"{label}: {value:.0f} токенов","tone":tone} for (label,value,tone),count in zip(segments,counts) for _ in range(count)]

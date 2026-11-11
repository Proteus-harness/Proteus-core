// Read-only request metadata. Never alter requests, responses, headers or streams.
import { appendFileSync } from "node:fs";
import { createHash } from "node:crypto";

const originalFetch = globalThis.fetch;
let attempt = 0;
const record = value => appendFileSync("/state/cap-wire.jsonl", JSON.stringify(value) + "\n");

globalThis.fetch = async function (input, options) {
    const address = typeof input === "string" ? input : input.url;
    if (address !== "https://opencode.ai/inference/go/openai/v1/chat/completions")
        return originalFetch.call(this, input, options);
    const request = JSON.parse(options.body);
    const current = ++attempt;
    record({ event: "request", attempt: current, model: request.model,
        reasoning_effort: request.reasoning_effort ?? null,
        max_tokens: request.max_tokens ?? null,
        max_completion_tokens: request.max_completion_tokens ?? null,
        request_body_sha256: createHash("sha256").update(options.body).digest("hex") });
    try {
        const response = await originalFetch.call(this, input, options);
        record({ event: "response", attempt: current, status: response.status });
        return response;
    } catch (error) {
        record({ event: "transport_error", attempt: current,
            name: error.name, code: error.code ?? error.cause?.code ?? null });
        throw error;
    }
};

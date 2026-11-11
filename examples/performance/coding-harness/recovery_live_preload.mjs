// Opt-in controlled live transport fault. Requests/responses stay real; no fixtures.
import { appendFileSync, readFileSync } from "node:fs";
import { createHash } from "node:crypto";

const originalFetch = globalThis.fetch;
const digest = value => createHash("sha256").update(JSON.stringify(value)).digest("hex");
const evidence = value => appendFileSync("/state/live-transport.jsonl", JSON.stringify(value) + "\n");
let attempt = 0;
let injected = false;
let faultHash;
const marker = "committed-live-effect\n";

globalThis.fetch = async (url, options) => {
    const address = typeof url === "string" ? url : url.url;
    if (address !== "https://opencode.ai/inference/go/openai/v1/chat/completions")
        throw new Error("Controlled live probe denied an unexpected endpoint");
    const request = JSON.parse(options.body);
    const current = ++attempt;
    const acceptedResults = request.messages.filter(message => message.role === "tool");
    const messagesToolsHash = digest({ messages: request.messages, tools: request.tools });
    let disk;
    try { disk = readFileSync("/workspace/append.log", "utf8"); } catch { disk = ""; }
    const eligible = !injected && acceptedResults.length > 0 && disk === marker
        && acceptedResults.some(message => JSON.stringify(message).includes("committed-live-effect"));
    evidence({ event: "request", attempt: current, model: request.model,
        reasoning_effort: request.reasoning_effort, messages_tools_sha256: messagesToolsHash,
        accepted_tool_result_ids: acceptedResults.map(message => message.tool_call_id),
        observed_disk_lines: disk.split("\n").filter(Boolean).length,
        retry_preserves_messages_tools: faultHash ? messagesToolsHash === faultHash : null });
    const response = await originalFetch(url, options);
    evidence({ event: "real_http_response", attempt: current, status: response.status,
        content_type: response.headers.get("content-type") });
    if (!response.ok || !response.body) return response;
    // Reserve the single injected fault only after actual successful HTTP/SSE
    // and an accepted tool result with its independently observed disk effect.
    const inject = eligible && response.headers.get("content-type")?.includes("text/event-stream");
    if (inject) { injected = true; faultHash = messagesToolsHash; }
    const reader = response.body.getReader();
    let bytes = 0;
    let cutPending = false;
    let tail = "";
    let sseDone = false;
    let cancelled = false;
    const hash = createHash("sha256");
    return new Response(new ReadableStream({
        async pull(controller) {
            if (cutPending) {
                evidence({ event: "injected_transport_cutoff", attempt: current,
                    real_bytes_forwarded: bytes, committed_disk_effect: disk === marker,
                    accepted_tool_result_ids: acceptedResults.map(message => message.tool_call_id),
                    messages_tools_sha256: messagesToolsHash });
                await reader.cancel("controlled live recovery fault").catch(() => {});
                controller.error(Object.assign(new Error("terminated: controlled live read cutoff (UND_ERR_SOCKET)"), { code: "UND_ERR_SOCKET" }));
                return;
            }
            try {
                const part = await reader.read();
                if (part.done) {
                    evidence({ event: "real_stream_completed", attempt: current, bytes,
                        response_body_sha256: hash.digest("hex") });
                    if (!cancelled) controller.close();
                } else {
                    // Keep the first actual network fragment incomplete even
                    // if the server coalesces the entire short answer in it.
                    const forwarded = inject ? part.value.subarray(0, Math.min(8, part.value.byteLength)) : part.value;
                    bytes += forwarded.byteLength;
                    hash.update(forwarded);
                    tail = (tail + new TextDecoder().decode(forwarded)).slice(-256);
                    if (!sseDone && tail.includes("data: [DONE]")) {
                        sseDone = true;
                        evidence({ event: "real_sse_done", attempt: current, bytes });
                    }
                    controller.enqueue(forwarded);
                    if (inject) cutPending = true;
                }
            } catch (error) {
                // An SDK may cancel immediately at [DONE] while the pending
                // underlying read resolves. This is a consumer lifecycle race,
                // not a provider or transport failure.
                if (sseDone && (cancelled || error.code === "ERR_INVALID_STATE")) {
                    evidence({ event: "consumer_closed_after_done", attempt: current });
                    return;
                }
                evidence({ event: "spontaneous_transport_error", attempt: current,
                    error_name: error.name, error_code: error.code ?? error.cause?.code ?? null });
                controller.error(error);
            }
        },
        async cancel(reason) { cancelled = true; await reader.cancel(reason); },
    }), { status: response.status, statusText: response.statusText, headers: response.headers });
};

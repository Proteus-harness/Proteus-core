// Offline-only fixture for the actual Pi provider. Every fetch is intercepted.
import { appendFileSync, readFileSync } from "node:fs";

let attempt = 0;
let previous;
const encoder = new TextEncoder();
const chunks = (delta, finish = null) => ({
    id: "recovery-response", object: "chat.completion.chunk", created: 0,
    model: "deepseek-v4.1-flash", choices: [{ index: 0, delta, finish_reason: finish }],
});
const encode = value => encoder.encode("data: " + JSON.stringify(value) + "\n\n");
const callID = "recovery-append-once";

globalThis.fetch = async (url, options) => {
    const address = typeof url === "string" ? url : url.url;
    if (address !== "https://opencode.ai/inference/go/openai/v1/chat/completions")
        throw new Error("Recovery fixture denied an unexpected network request");
    const request = JSON.parse(options.body);
    const { name: tool } = request.tools.find(item => ["bash", "shell", "exec_command"].includes(item.function?.name)).function;
    attempt++;
    appendFileSync("/state/requests.jsonl", JSON.stringify({ attempt, request }) + "\n");
    let delta;
    let finish;
    if (attempt === 1) {
        const code = "from pathlib import Path; p=Path('counter.txt'); p.write_text(str(int(p.read_text())+1)); print('effect-count='+p.read_text())";
        const args = { [tool === "exec_command" ? "cmd" : "command"]: "python3 -c '" + code.replaceAll("'", "'\\''") + "'" };
        if (tool === "shell") Object.assign(args, { description: "Increment recovery counter", background: false, timeout: 5000 });
        delta = { role: "assistant", tool_calls: [{ index: 0, id: callID, type: "function", function: { name: tool, arguments: JSON.stringify(args) } }] };
        finish = "tool_calls";
    } else {
        const results = request.messages.filter(message => message.role === "tool" && message.tool_call_id === callID);
        if (results.length !== 1 || !JSON.stringify(results[0]).includes("effect-count=1"))
            throw new Error("Missing the exactly-once committed result on continuation");
        if (readFileSync("/workspace/counter.txt", "utf8") !== "1") throw new Error("Repeated OS effect");
        if (previous && JSON.stringify(request.messages) !== JSON.stringify(previous.messages))
            throw new Error("Retry altered accepted native model history");
        previous = request;
        if (attempt === 2) {
            return new Response(new ReadableStream({
                async start(controller) {
                    controller.enqueue(encode(chunks({ role: "assistant", content: "UNCOMMITTED_DRAFT" })));
                    controller.enqueue(encode(chunks({ tool_calls: [{ index: 0, id: "recovery-partial-call", type: "function", function: {
                        name: tool, arguments: '{"cmd":"touch partial-marker.txt"',
                    } }] })));
                    await new Promise(resolve => setTimeout(resolve, 100));
                    controller.error(Object.assign(new Error("terminated: other side closed (UND_ERR_SOCKET)"), { code: "UND_ERR_SOCKET" }));
                },
            }), { headers: { "content-type": "text/event-stream" } });
        }
        if (attempt !== 3) throw new Error("Unexpected additional provider request");
        delta = { role: "assistant", content: "RECOVERY_OK" };
        finish = "stop";
    }
    const data = [chunks(delta), { ...chunks({}, finish), usage: { prompt_tokens: 100, completion_tokens: 10, total_tokens: 110 } }];
    return new Response(new ReadableStream({ start(controller) {
        for (const part of data) controller.enqueue(encode(part));
        controller.enqueue(encoder.encode("data: [DONE]\n\n"));
        controller.close();
    } }), { headers: { "content-type": "text/event-stream" } });
};

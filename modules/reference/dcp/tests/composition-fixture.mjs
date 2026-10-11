import { randomUUID } from "node:crypto";
import { mkdtemp, rm, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
export const part = (payload) => ({ part_id: randomUUID(), provenance: "model", scope: "conversation", payload });
export const message = (role, value) => ({ id: randomUUID(), role, phase: null, name: null, tool_call_id: null,
  metadata: {}, parts: Array.isArray(value) ? value : [part({ Text: { text: value } })] });
export const call = (name, args = {}) => ({ id: randomUUID(), name, args, surface: "function", raw_arguments: null });
export const contents = (messages) => messages.flatMap((m) => m.parts.map((p) => p.payload.Text?.text ?? p.payload.ToolResult?.result.output ?? "")).join("\n");
export const signal = () => new AbortController().signal;
export async function fixture(t, messages) {
  const directory = await mkdtemp(join(tmpdir(), "proteus-dcp-composition-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const attribution = { execution_id: randomUUID(), agent: { session_id: randomUUID(), thread_id: randomUUID(), turn_id: randomUUID() } };
  return { directory, attribution, messages, file: join(directory, `${attribution.agent.session_id}.json`) };
}
export const state = async (f) => JSON.parse(await readFile(f.file, "utf8")).state;
export const hook = (module, f) => module.hook({ cwd: "/tmp", attribution: f.attribution,
  conversation: { messages: f.messages, model_context: [] }, event: { event: "before_model", origin: "direct",
    request: { model: { provider: "fake", model: "fake" }, messages: f.messages, tools: [module.spec], instructions: [],
      limits: { max_input_tokens: 128000, max_output_tokens: null } } } }, signal());
export const compress = (module, f, args) => module.invoke({ cwd: "/tmp", attribution: f.attribution,
  skills: { disabled: [], packages: [] }, call: { ...f.messages.at(-1).parts[0].payload.ToolCall.call, args } },
  async () => ({ conversation: { messages: f.messages, model_context: [] }, message_id: f.messages.at(-1).id }), signal());
export const range = (startId, endId, summary) => ({ topic: "done", content: [{ startId, endId, summary }] });
export const toolResult = (c, output) => message("Tool", [part({ ToolResult: { result: {
  call_id: c.id, ok: true, output, content: [], error: null, metadata: null } } })]);

import test from "node:test";
import assert from "node:assert/strict";
import { createDcp } from "../dist/module.js";
import { part, message, call, fixture, hook, compress, range, toolResult, state } from "./composition-fixture.mjs";

test("retained failed tool age is reconstructed after prefix compaction for warm and cold error pruning", async (t) => {
  const failed = call("read_file", { path: "failed-path.txt" });
  const result = toolResult(failed, "");
  Object.assign(result.parts[0].payload.ToolResult.result, { ok: false, error: "ERROR DETAIL" });
  const prefix = [message("User", "old user"), ...Array.from({ length: 8 }, (_, i) => message("Assistant", `old prefix ${i}`))];
  const tail = [message("Assistant", [part({ ToolCall: { call: failed } })]), result, message("User", "CURRENT USER")];
  const outcomes = [];
  for (const cold of [false, true]) {
    const f = await fixture(t, structuredClone([...prefix, ...tail]));
    let module = createDcp({ state_dir: f.directory });
    await hook(module, f); // The failed call initially sits at upstream turn 9.
    const checkpoint = message("Assistant", "NEW COMPACTOR SUMMARY");
    checkpoint.parts[0].provenance = "compactor";
    f.messages = [checkpoint, ...structuredClone(tail), ...Array.from({ length: 4 }, (_, i) => message("Assistant", `recent ${i}`)),
      message("Assistant", [part({ ToolCall: { call: call("compress") } })])];
    const canonical = structuredClone(f.messages);
    if (cold) module = createDcp({ state_dir: f.directory });
    await hook(module, f);
    // Compression runs the pinned automatic error-purge strategy. Its turn age
    // must use the retained suffix positions, including a warm cached call.
    await compress(module, f, range("@4@", "@4@", "ONE ITEM SUMMARY"));
    const output = await hook(module, f);
    const transformed = output.messages.flatMap((m) => m.parts).find((p) => p.payload.ToolCall?.call.id === failed.id);
    assert.equal(transformed.payload.ToolCall.call.args.path, "[input removed due to failed tool call]");
    assert.deepEqual(f.messages, canonical);
    const persisted = await state(f);
    assert.ok(persisted.prune.tools[failed.id] > 0);
    outcomes.push({ args: transformed.payload.ToolCall.call.args, prunedTools: persisted.prune.tools });
  }
  assert.deepEqual(outcomes[0], outcomes[1]);
});

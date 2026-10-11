import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { writeFile } from "node:fs/promises";
import { createDcp } from "../dist/module.js";
import { part, message, call, contents, fixture, hook, compress, range, toolResult, state } from "./composition-fixture.mjs";
const patch = "*** Begin Patch\n*** Update File: protected.txt\n@@\n-before\n+after\n*** End Patch";

for (const [name, args, surface, preserved] of [
  ["read", { path: "protected.txt" }, "function", true],
  ["read_file", { path: "protected.txt" }, "function", true],
  ["write_file", { path: "protected.txt", content: "IMPORTANT DATA" }, "function", true],
  ["edit_file", { path: "protected.txt", old_string: "before", new_string: "after" }, "function", true],
  ["read_many_files", { paths: ["unprotected.txt", "protected.txt"] }, "function", true],
  ["apply_patch", { patch: "*** Begin Patch\n*** Update File: unprotected.txt\n*** Move to: protected.txt\n@@\n-before\n+after\n*** End Patch" }, "function", true],
  ["apply_patch", { patch }, "function", true],
  ["apply_patch", { input: patch }, "freeform", true],
  ["apply_patch", { patchText: patch }, "function", true],
  ["external_reader", { path: "protected.txt" }, "function", false],
]) test(`file protection adapts ${name}/${Object.keys(args)[0]} without changing canonical arguments`, async (t) => {
  const c = { ...call(name, args), surface, raw_arguments: JSON.stringify(args) };
  const f = await fixture(t, [message("User", "inspect"), message("Assistant", [part({ ToolCall: { call: c } })]),
    toolResult(c, "EXACT PROTECTED TOOL OUTPUT"), message("User", "continue"), message("Assistant", [part({ ToolCall: { call: call("compress") } })])]);
  const original = structuredClone(f.messages);
  const module = createDcp({ state_dir: f.directory, protectedFilePatterns: ["protected.txt"] });
  await hook(module, f);
  await compress(module, f, range("@1@", "@2@", "Generic summary."));
  const result = await hook(module, f);
  assert.equal(contents(result.messages).includes("EXACT PROTECTED TOOL OUTPUT"), preserved);
  assert.deepEqual(f.messages, original);
  // A second uncompressed file call remains byte-for-byte canonical after the bridge.
  const repeated = { ...c, id: call(name, args).id };
  f.messages.push(message("Assistant", [part({ ToolCall: { call: repeated } })]));
  const untouched = (await hook(module, f)).messages.at(-1).parts.find((p) => p.payload.ToolCall);
  assert.deepEqual(untouched, f.messages.at(-1).parts[0]);
});

test("actual tool names remain the protection authority", async (t) => {
  const c = call("read_file", { path: "unprotected.txt" });
  const f = await fixture(t, [message("User", "inspect"), message("Assistant", [part({ ToolCall: { call: c } })]),
    toolResult(c, "ACTUAL NAME PROTECTED"), message("User", "continue"), message("Assistant", [part({ ToolCall: { call: call("compress") } })])]);
  const module = createDcp({ state_dir: f.directory, compress: { protectedTools: ["read_file"] } });
  await hook(module, f);
  await compress(module, f, range("@1@", "@2@", "summary"));
  assert.match(contents((await hook(module, f)).messages), /ACTUAL NAME PROTECTED/);
});

async function compressed(t) {
  const f = await fixture(t, [message("User", "old start"), message("Assistant", "OLD MIDDLE IMPORTANT"),
    message("User", "KEPT USER PROMPT"), message("Assistant", [part({ ToolCall: { call: call("compress") } })])]);
  const module = createDcp({ state_dir: f.directory });
  await hook(module, f);
  await compress(module, f, range("@1@", "@3@", "DCP OLD SUMMARY"));
  return { f, module };
}

test("prefix checkpoint replacement has the same safe view before and after restart", async (t) => {
  const { f, module } = await compressed(t);
  for (let iteration = 0; iteration < 2; iteration++) {
    const summary = message("Assistant", `COMPACTOR SUMMARY ${iteration}`);
    summary.parts[0].provenance = "compactor";
    f.messages = [summary, ...f.messages.slice(1)];
    const canonical = structuredClone(f.messages);
    // Cold first: reproduces initialization-before-checkpoint-reset ordering.
    const cold = await hook(createDcp({ state_dir: f.directory }), f);
    const warm = await hook(module, f);
    assert.match(contents(cold.messages), /OLD MIDDLE IMPORTANT/);
    assert.match(contents(warm.messages), /KEPT USER PROMPT/);
    assert.equal(contents(cold.messages), contents(warm.messages));
    assert.deepEqual(f.messages, canonical);
    const persisted = await state(f);
    assert.deepEqual(persisted.prune.messages.activeBlockIds, []);
    assert.deepEqual(persisted.prune.messages.activeByAnchorMessageId, {});
    assert.deepEqual(persisted.prune.messages.byMessageId, {});
  }
});

test("tracked python_suffix keeps its retained user prompt after DCP and cold resume", async (t) => {
  const { f, module } = await compressed(t);
  const c = f.messages.at(-1).parts[0].payload.ToolCall.call;
  f.messages.push(toolResult(c, "Compressed 3 messages"), message("Assistant", "CONTROLLER CONTINUES"));
  const retainedId = f.messages[2].id;
  const path = fileURLToPath(new URL("../../../../examples/modules/context-preparation-process/prepare.py", import.meta.url));
  const py = `import runpy,sys,json; f=runpy.run_path(sys.argv[1])['compact']; print(json.dumps(f(json.load(sys.stdin), {"trigger_messages":3,"retain_user_turns":1})))`;
  const compacted = JSON.parse(execFileSync("python3", ["-B", "-c", py, path], { input: JSON.stringify({
    messages: f.messages, request: null, token_estimate: 1000,
    window_tokens: 2000, reason: "manual" }) }));
  assert.equal(compacted.output.changed, true);
  assert.ok(compacted.output.messages.some((m) => m.id === retainedId));
  f.messages = compacted.output.messages;
  const canonical = structuredClone(f.messages);
  const warm = await hook(module, f);
  const cold = await hook(createDcp({ state_dir: f.directory }), f);
  assert.match(contents(warm.messages), /KEPT USER PROMPT/);
  assert.doesNotMatch(contents(warm.messages), /DCP OLD SUMMARY/);
  assert.equal(contents(cold.messages), contents(warm.messages));
  assert.deepEqual(f.messages, canonical);
});

test("dangling block members cannot prune a surviving anchor and malformed summaries cannot remove messages", async (t) => {
  for (const mutation of ["missing-member", "empty-summary", "missing-origin-call"]) {
    const { f } = await compressed(t);
    const persisted = await state(f);
    if (mutation === "missing-member") f.messages.splice(1, 1);
    if (mutation === "empty-summary") persisted.prune.messages.blocksById["1"].summary = "";
    if (mutation === "missing-origin-call") f.messages.at(-1).parts[0].payload.ToolCall.call.id = "changed-call";
    await writeFile(f.file, JSON.stringify({ version: 1, state: persisted }));
    const cold = await hook(createDcp({ state_dir: f.directory }), f);
    assert.match(contents(cold.messages), /old start/);
    assert.match(contents(cold.messages), /KEPT USER PROMPT/);
    assert.deepEqual((await state(f)).prune.messages.activeBlockIds, []);
  }
});


test("history replacement preserves valid nested suffix blocks and reactivates a child when only its parent is invalid", async (t) => {
  const f = await fixture(t, [message("User", "discard prefix"), message("Assistant", "obsolete prefix"),
    message("User", "nested start"), message("Assistant", "nested research"), message("User", "parent-only member"),
    message("Assistant", [part({ ToolCall: { call: call("compress") } })])]);
  const module = createDcp({ state_dir: f.directory });
  await hook(module, f);
  await compress(module, f, range("@3@", "@4@", "CHILD SUMMARY"));
  f.messages.push(message("Assistant", [part({ ToolCall: { call: call("compress") } })]));
  await hook(module, f);
  await compress(module, f, range("@b1@", "@5@", "NESTED PARENT includes @b1@"));
  const prior = await state(f);
  assert.deepEqual(prior.prune.messages.activeBlockIds, [2]);
  const checkpoint = message("Assistant", "compactor replaces unrelated prefix");
  checkpoint.parts[0].provenance = "compactor";
  f.messages = [checkpoint, ...f.messages.slice(2)];
  const warm = await hook(module, f);
  const cold = await hook(createDcp({ state_dir: f.directory }), f);
  assert.match(contents(warm.messages), /NESTED PARENT/);
  assert.match(contents(warm.messages), /CHILD SUMMARY/);
  assert.equal(contents(warm.messages), contents(cold.messages));
  assert.deepEqual((await state(f)).prune.messages.activeBlockIds, [2]);
  // Removing a parent-only source member invalidates the parent, not its child.
  f.messages = f.messages.filter((m) => !contents([m]).includes("parent-only member"));
  const child = await hook(module, f);
  const resumed = await hook(createDcp({ state_dir: f.directory }), f);
  assert.match(contents(child.messages), /CHILD SUMMARY/);
  assert.doesNotMatch(contents(child.messages), /NESTED PARENT/);
  assert.equal(contents(child.messages), contents(resumed.messages));
  const after = await state(f);
  assert.deepEqual(after.prune.messages.activeBlockIds, [1]);
  assert.deepEqual(after.prune.messages.blocksById[1].parentBlockIds, []);
  assert.equal(after.prune.messages.nextBlockId, prior.prune.messages.nextBlockId);
  assert.equal(after.prune.messages.nextRunId, prior.prune.messages.nextRunId);
  assert.equal(after.stats.totalPruneTokens, prior.stats.totalPruneTokens);
  assert.ok(Object.values(after.prune.messages.byMessageId).every((entry) => entry.allBlockIds.every((id) => id === 1) && entry.activeBlockIds.every((id) => id === 1)));
});

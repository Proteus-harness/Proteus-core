import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { once } from "node:events";
import { test } from "node:test";

const worker = new URL("../worker.mjs", import.meta.url);

test("real TS worker multiplexes exports, redirects logs and cancels only the addressed invocation", { timeout: 5000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), "proteus-hooks-"));
  const entry = join(directory, "hook.ts");
  await writeFile(entry, `import {setTimeout} from 'node:timers/promises';
    export default function(hooks: {on: Function, config: {tag: string}}) {
      console.log('ported log');
      hooks.on('before_context', async (event: any, ctx: any) => {
        const model = await ctx.completeModel({messages: []});
        const tool = await ctx.executeTool({id:'nested', name:'read_file',args:{path:'a'}});
        return {action:'context_preparation', output:{...event.output,metadata:{model,tool}}};
      });
      hooks.on('before_tool', async (event: any, ctx: any) => {
        if(event.call.name==='wait') await setTimeout(2000, undefined, {signal:ctx.signal});
        return {action:'block_tool',reason:hooks.config.tag};
      });
    }`);
  const child = spawn(process.execPath, [worker.pathname], { stdio: ["pipe", "pipe", "pipe"] });
  const closed = once(child, "close");
  let stderr = "";
  child.stderr.setEncoding("utf8").on("data", (text) => { stderr += text; });
  const replies = new Map();
  const callbacks = [];
  let failModel = false;
  const modelFailure = {kind:"context_window_exceeded",message:"fixture overflow",completed_messages:[]};
  const lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    const frame = JSON.parse(line);
    if (frame.id.startsWith("m:")) {
      callbacks.push(frame);
      const reply = failModel && frame.method === "host.model.complete"
        ? {error:{code:-32100,message:"fixture model failed",data:modelFailure}}
        : {result:{value:frame.method}};
      child.stdin.write(`${JSON.stringify({jsonrpc:"2.0",id:frame.id,...reply})}\n`);
      return;
    }
    const waiter = replies.get(frame.id);
    if (waiter) { replies.delete(frame.id); waiter(frame); }
  });
  const send = (id, method, params) => new Promise((resolve) => {
    replies.set(id, resolve);
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
  });
   const binding = (id) => ({ slot: "hook", module_id: id, contract_version: "v6", composition: "ordered_many", module_config: { entry, settings: { tag: id } }, host_features: [] });
  const invocation = (id, module, name) => ({
    export: { slot: "hook", module_id: module },
    lineage: { root_invocation_id: id, parent_invocation_id: null, depth: 0 },
    params: { cwd: directory, attribution: { execution_id: "e", agent: null }, conversation: null,
      event: { event: "before_tool", call: { id: "c", name, args: {} }, spec: null, blocked: null } },
  });
  try {
    const initialized = await send("h:1:0", "initialize", { protocol_version: "v3", component_id: "js", exports: [binding("a"), binding("b")] });
    assert.equal(initialized.result.exports.length, 2);
    const slow = send("h:1:1", "hook.invoke", invocation("h:1:1", "a", "wait"));
    const fast = await send("h:1:2", "hook.invoke", invocation("h:1:2", "b", "read"));
    assert.equal(fast.result.result.reason, "b");
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "$/cancelRequest", params: { invocation_id: "h:1:1", cause: "user" } })}\n`);
    assert.equal((await slow).error.code, -32800);
    assert.equal((await send("h:1:3", "hook.invoke", invocation("h:1:3", "a", "read"))).result.result.reason, "a");
    const stale = await send("h:0:4", "hook.invoke", invocation("h:0:4", "a", "read"));
    assert.match(stale.error.message, /stale generation/);
    const denied = await send("h:1:4", "host.tools.invoke", invocation("h:1:4", "a", "read"));
    assert.match(denied.error.message, /unsupported hook method/);
    assert.match(stderr, /ported log/);
    const preparation = invocation("h:1:5", "a", "read");
    preparation.params.event = {event:"before_context",input:{messages:[],request:null,token_estimate:null,window_tokens:null,reason:null},output:{messages:[],changed:false},target:null};
    const prepared = await send("h:1:5", "hook.invoke", preparation);
    assert.equal(prepared.result.result.action, "context_preparation");
    assert.deepEqual(callbacks.map(frame=>frame.method), ["host.model.complete", "host.tools.execute"]);
    assert.ok(callbacks.every(frame=>frame.params.invocation_id==="h:1:5"));
    assert.equal(prepared.result.result.output.metadata.model.value, "host.model.complete");
    failModel = true;
    preparation.lineage.root_invocation_id = "h:1:6";
    const failed = await send("h:1:6", "hook.invoke", preparation);
    assert.equal(failed.error.code, -32000);
    assert.deepEqual(failed.error.data, modelFailure);
  } finally {
    child.stdin.end();
    await closed;
    lines.close();
    await rm(directory, { recursive: true });
  }
});

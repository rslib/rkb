// Drives extensions/rkb.ts against a fake pi/omp API with the real `rkb` binary on PATH.
// Usage: node run.mjs <generated extension .ts> <pi|omp>
// Env: XDG_STATE_HOME and RKB_HOME set up by the Rust test; RKB_KB is the knowledge base folder;
// RKB_REQUEST and RKB_QUESTION name a stored request.
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";

const [file, harness] = process.argv.slice(2);
const handlers = {};
const commands = {};
const sent = [];
const mod = await import(file);
mod.default({
  on: (name, fn) => (handlers[name] = fn),
  registerCommand: (name, options) => (commands[name] = options),
  sendUserMessage: (content) => sent.push(content),
});

assert.equal(process.env.RKB_HARNESS, harness);
const stopEvent = harness === "pi" ? "agent_before_settle" : "session_stop";
const other = harness === "pi" ? "session_stop" : "agent_before_settle";
assert.ok(handlers[stopEvent] && !handlers[other], `registers only ${stopEvent}`);

const session = `s-${harness}`;
let answer = true;
let asked = [];
const ctx = (hasUI = true) => ({
  cwd: process.env.RKB_KB,
  hasUI,
  sessionManager: { getSessionId: () => session, getSessionFile: () => process.env.RKB_TRANSCRIPT },
  ui: { confirm: async (title, message) => (asked.push(message), answer) },
});
const on = (name, event, c = ctx()) => handlers[name](event, c);
const bash = (command) => ({ toolName: "bash", input: { command } });
const signals = () =>
  readFileSync(`${process.env.XDG_STATE_HOME}/rkb/sessions/${session}.jsonl`, "utf8")
    .trim()
    .split("\n")
    .map((l) => JSON.parse(l));

// Session start: the context lines go in once, hidden, with the first prompt.
await on("session_start", { reason: "startup" });
const first = await on("before_agent_start", { prompt: "hi" });
assert.equal(first.message.customType, "rkb");
assert.equal(first.message.display, false);
assert.match(first.message.content, /^rkb: /);
assert.equal(await on("before_agent_start", { prompt: "again" }), undefined);

// Other tools and commands without rkb are left alone.
assert.equal(await on("tool_call", { toolName: "read", input: { path: "x" } }), undefined);
assert.equal(await on("tool_call", bash("ls -la")), undefined);
assert.equal(await on("tool_result", { toolName: "read", input: {}, content: [], isError: true }), undefined);

// A failed linker command gets the lesson appended and stays an error.
const linker =
  "/usr/bin/ld: main.o: in function `main':\nmain.cpp:(.text+0x1f): undefined reference to `vtable for Widget'\ncollect2: error: ld returned 1 exit status\n\nCommand exited with code 1";
const failed = { ...bash("g++ main.o -o app"), content: [{ type: "text", text: linker }], isError: true };
const result = await on("tool_result", failed);
assert.equal(result.content.length, 2);
assert.equal(result.content[0].text, linker);
assert.match(result.content[1].text, /1a00000012/);
assert.equal(result.isError, undefined, "the error state is kept");
assert.equal(await on("tool_result", failed), undefined, "once per session");

// Failed then fixed, and prompt signals without prompt text.
await on("tool_result", { ...bash("cmake -B build"), content: [{ type: "text", text: "odd\n\nCommand exited with code 1" }], isError: true });
assert.equal(await on("tool_result", { ...bash("cmake -B build -DX=1"), content: [{ type: "text", text: "ok" }], isError: false }), undefined);
const inputReply = await on("input", { text: "actually, use the release build and remember this", source: "interactive" });
assert.deepEqual(inputReply, harness === "pi" ? { action: "continue" } : undefined);
const kinds = signals().map((s) => s.kind);
assert.deepEqual(kinds, ["failed", "injected", "failed", "failed", "fixed", "correction", "remember"]);
assert.ok(!JSON.stringify(signals()).includes("release"));

// Compaction saves an extract, because the session has a fixed signal; shutdown then finds nothing new.
const inbox = () => {
  try {
    return readdirSync(`${process.env.XDG_STATE_HOME}/rkb/inbox`).filter((f) => f.endsWith(".md"));
  } catch {
    return [];
  }
};
const before = inbox().length;
assert.equal(await on("session_before_compact", { reason: "threshold" }), undefined, "the compaction is not changed");
assert.equal(inbox().length, before + 1);
const item = readFileSync(`${process.env.XDG_STATE_HOME}/rkb/inbox/${inbox().find((f) => !f.startsWith("."))}`, "utf8");
assert.match(item, /\[command failed, exit 1\] cmake -B build/);
await on("session_shutdown", { reason: "quit" });
assert.equal(inbox().length, before + 1, "nothing new at shutdown");
const noFile = { ...ctx(), sessionManager: { getSessionId: () => session, getSessionFile: () => undefined } };
await on("session_before_compact", {}, noFile);
assert.equal(inbox().length, before + 1, "an ephemeral session is not forwarded");

// The two commands send their prompts as user messages.
assert.deepEqual(Object.keys(commands).sort(), ["rkb-distill", "rkb-retro"]);
await commands["rkb-retro"].handler("the cmake part", ctx());
assert.match(sent.at(-1), /^Review the work of this session/);
assert.match(sent.at(-1), /Focus from the user, if any: the cmake part/);
await commands["rkb-distill"].handler("", ctx());
assert.match(sent.at(-1), /^Turn rkb inbox items into lessons/);
assert.match(sent.at(-1), /the ids the user gave: none/);

// Stop nudge: once, then quiet while the continuation settles.
const stop = (active) =>
  harness === "pi"
    ? { outcome: "completed", entries: [], continue: false, context: { canContinue: true } }
    : { session_id: session, stop_hook_active: active, signal: { aborted: false } };
const nudge = await on(stopEvent, stop(false));
assert.equal(nudge.continue, true);
const note = harness === "pi" ? nudge.entries[0].content : nudge.additionalContext;
assert.match(note, /cmake/);
if (harness === "pi") assert.deepEqual([nudge.entries[0].type, nudge.entries[0].display], ["custom_message", false]);
// A new signal while the nudge's own continuation settles.
await on("tool_result", { ...bash("ninja"), content: [{ type: "text", text: "e\n\nCommand exited with code 1" }], isError: true });
await on("tool_result", { ...bash("ninja"), content: [{ type: "text", text: "ok" }], isError: false });
if (harness === "pi") {
  assert.equal(await on(stopEvent, stop()), undefined, "continued by rkb, so the extension sends stop_hook_active");
} else {
  assert.equal(await on(stopEvent, stop(true)), undefined, "omp says a stop hook is active");
}

// Confirm dialog.
const confirm = bash(`sh -c 'rkb confirm ${process.env.RKB_REQUEST} --choice "install"'`);
answer = true;
assert.equal(await on("tool_call", confirm), undefined);
assert.ok(asked.at(-1).includes(process.env.RKB_QUESTION) && asked.at(-1).includes("choice: install"), asked.at(-1));
answer = false;
assert.match((await on("tool_call", confirm)).reason, /declined/);
const blocked = await on("tool_call", confirm, ctx(false));
assert.equal(blocked.block, true);
assert.match(blocked.reason, /terminal/);

// Without rkb: results are unchanged and the gate still holds.
const path = process.env.PATH;
process.env.PATH = "/nonexistent";
assert.equal(await on("tool_result", { ...bash("make"), content: [{ type: "text", text: "error: x\n\nCommand exited with code 2" }], isError: true }), undefined);
asked = [];
answer = true;
assert.equal(await on("tool_call", bash("rkb confirm r-000000 --choice x")), undefined);
assert.equal(asked.length, 1, "the dialog is still shown");
assert.equal((await on("tool_call", bash("rkb confirm r-000000 --choice x"), ctx(false))).block, true);
assert.equal(await on("tool_call", bash("rkb show 1a00000012")), undefined);
process.env.PATH = path;

console.log(`ok ${harness}`);

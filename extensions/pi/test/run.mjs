// Drives extensions/pi/src/rkb.ts against a fake pi/omp API with the real `rkb` binary on PATH.
// Usage: node run.mjs <generated extension .ts> <pi|omp>
// Env: XDG_STATE_HOME and RKB_HOME set up by the Rust test; RKB_KB is the knowledge base folder;
// RKB_REQUEST and RKB_QUESTION name a stored request.
import assert from "node:assert/strict";
import { chmodSync, existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { visibleWidth } from "@earendil-works/pi-tui";

const [file, harness] = process.argv.slice(2);
const handlers = {};
const commands = {};
const tools = {};
const sent = [];
const mod = await import(file);
mod.default({
  on: (name, fn) => (handlers[name] = fn),
  registerCommand: (name, options) => (commands[name] = options),
  registerTool: (def) => (tools[def.name] = def),
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
// omp runs second on the same machine state, so it sees pi's errors again and records `repeat`.
const expected = harness === "omp"
  ? ["failed", "repeat", "injected", "failed", "failed", "repeat", "nohit", "fixed", "inferred", "correction", "remember"]
  : ["failed", "injected", "failed", "failed", "nohit", "fixed", "inferred", "correction", "remember"];
assert.deepEqual(kinds, expected);
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
if (harness === "omp") {
  // omp names the session file in its stop event; the hooks then find the transcript without the session manager.
  await on("session_stop", { session_file: process.env.RKB_TRANSCRIPT, session_id: session, stop_hook_active: true });
  await on("tool_result", { ...bash("ninja"), content: [{ type: "text", text: "boom\n\nCommand exited with code 1" }], isError: true });
  await on("tool_result", { ...bash("ninja -j4"), content: [{ type: "text", text: "ok" }], isError: false });
  const extracted = () => signals().filter((r) => r.kind === "extracted").length;
  const marks = extracted();
  await on("session_before_compact", {}, noFile);
  assert.equal(extracted(), marks + 1, "the stop event's session file is read");
  const other = { ...ctx(), sessionManager: { getSessionId: () => "someone-else", getSessionFile: () => undefined } };
  await on("tool_result", { ...bash("ninja"), content: [{ type: "text", text: "boom\n\nCommand exited with code 1" }], isError: true }, other);
  await on("tool_result", { ...bash("ninja -j4"), content: [{ type: "text", text: "ok" }], isError: false }, other);
  await on("session_before_compact", {}, other);
  assert.ok(!existsSync(`${process.env.XDG_STATE_HOME}/rkb/sessions/someone-else.jsonl`) || !readFileSync(`${process.env.XDG_STATE_HOME}/rkb/sessions/someone-else.jsonl`, "utf8").includes("extracted"), "another session never gets this session's file");
}

// The two commands send their prompts as user messages.
assert.deepEqual(Object.keys(commands).sort(), ["rkb-curate", "rkb-distill", "rkb-retro", "rkb-status"]);
await commands["rkb-retro"].handler("the cmake part", ctx());
assert.match(sent.at(-1), /^Review the work of this session/);
assert.match(sent.at(-1), /Focus from the user, if any: the cmake part/);
await commands["rkb-distill"].handler("", ctx());
assert.match(sent.at(-1), /^Distill the rkb inbox into lessons/);
assert.match(sent.at(-1), /the item ids the user gave, if any: none/);

// The agent tools run `rkb tool` and return its TOON text; failures throw, as pi expects.
assert.deepEqual(Object.keys(tools).sort(), ["rkb_add", "rkb_edit", "rkb_flag", "rkb_note", "rkb_search", "rkb_show", "rkb_used"]);
assert.match(tools.rkb_search.description, /before a web search/i);
assert.equal(tools.rkb_search.parameters.properties.query.type, "string");
const found = await tools.rkb_search.execute("t1", { query: "undefined reference to vtable" });
assert.match(found.content[0].text, /1a00000012/);
// The same run's JSON is the structured result a pi codemode script reads.
assert.ok(found.structuredContent.results.some((r) => r.id === "1a00000012"));
assert.ok(!found.content[0].text.startsWith("{"), "the model reads TOON, not JSON");
assert.equal(tools.rkb_search.namespace.name, "rkb");
assert.equal(tools.rkb_search.annotations.readOnlyHint, true);
assert.equal(tools.rkb_edit.annotations.destructiveHint, true);
assert.equal(tools.rkb_search.outputSchema.properties.results.type, "array");
assert.match(tools.rkb_search.promptGuidelines[0], /rkb_used/);
assert.equal(tools.rkb_show.promptGuidelines, undefined);
await assert.rejects(tools.rkb_show.execute("t2", {}), /`id` is required/);

// A recall reply goes into the next turn's context, hidden, with the session-start lines.
const stubDir = mkdtempSync(`${tmpdir()}/rkb-stub-`);
const recallNote = "rkb: lessons that may help with this message";
writeFileSync(
  `${stubDir}/rkb`,
  `#!/bin/sh\ncat >/dev/null\n[ "$2" = prompt ] && echo '{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"${recallNote}"}}'\nexit 0\n`,
);
chmodSync(`${stubDir}/rkb`, 0o755);
const realPath = process.env.PATH;
process.env.PATH = `${stubDir}:${realPath}`;
await on("input", { text: "why does the linker fail on vtable", source: "interactive" });
const recalled = await on("before_agent_start", { prompt: "why does the linker fail on vtable" });
assert.equal(recalled.message.display, false);
assert.ok(recalled.message.content.includes(recallNote), recalled.message.content);
assert.equal(await on("before_agent_start", { prompt: "next" }), undefined, "once");
process.env.PATH = realPath;

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

// The UI: a stub `rkb` serves canned JSON from files and logs its calls, so no knowledge base is touched.
const stub = mkdtempSync(`${tmpdir()}/rkb-ui-`);
writeFileSync(
  `${stub}/rkb`,
  `#!/bin/sh
D=$(dirname "$0")
in=$(cat)
echo "$* <<< $in" >> "$D/log"
case "$1 $2" in
  "inbox show"|"inbox triage") n="inbox-$2" ;;
  job\\ *) n="job-$2-$3" ;;
  *) n="$1" ;;
esac
[ -f "$D/$n.json" ] || exit 1
cat "$D/$n.json"
`,
);
chmodSync(`${stub}/rkb`, 0o755);
const canned = (name, value) => writeFileSync(`${stub}/${name}.json`, typeof value === "string" ? value : JSON.stringify(value));
const log = () => (existsSync(`${stub}/log`) ? readFileSync(`${stub}/log`, "utf8") : "");
const until = async (cond, what) => {
  for (let i = 0; i < 100; i++) {
    if (cond()) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  assert.fail(`timed out: ${what}`);
};
const quiet = (ms = 300) => new Promise((r) => setTimeout(r, ms));
canned("status", {
  inbox: { total: 12, priority: { 1: 8, 2: 1, 3: 3 } },
  curate: 0,
  requests: [{ id: "r-1a2b", question: "Install the rkb plugin", options: ["yes", "no"] }],
  injected: [{ id: "2b00000034", title: "Stale build cache", hook: "prompt" }],
});
canned("search", { results: [{ id: "1a00000012", title: "Linker undefined reference", relevance: "0.92", type: "pitfall", summary: "Link the library after the objects.", applies: "likely" }] });
canned("show", { title: "Linker undefined reference", body: "# Linker undefined reference\n\nPut -lfoo after main.o.", frontmatter: { type: "pitfall", status: "active" }, path: "general/cmake/linker.md", applies: { result: "likely" } });
const longBody = Array.from({ length: 80 }, (_, i) => `line ${i} of a long inbox note`).join("\n");
canned("inbox", {
  items: [
    { id: "cd236c6b0b", kind: "note", priority: 3, age: "2 min", preview: "make fails with a stale cache", verdicts: { keep: 1, known: 0, unsure: 0, drop: 0 } },
    { id: "ab12345678", kind: "observed", priority: 1, age: "1 h", preview: "prefer ninja over make", verdicts: null },
  ],
});
canned("inbox-show", { body: longBody, meta: { cwd: "/work/proj" } });
canned("inbox-triage", { total: { keep: 1, known: 0, unsure: 1, drop: 0 } });
canned("used", {});

// Earlier steps may still refresh in the background; let them finish before the stub takes over.
await new Promise((r) => setTimeout(r, 1000));
const uiPath = process.env.PATH;
process.env.PATH = `${stub}:${uiPath}`;
const uiCalls = [];
const shown = [];
let selected = "yes";
let popup;
const fakeTui = { terminal: { rows: 30 }, requestRender() {} };
const fakeTheme = { fg: (_c, t) => t, bold: (t) => t };
const complete = [];
const uiCtx = (over = {}) => ({
  ...ctx(),
  model: { provider: "fake", id: "m1" },
  modelRegistry: {
    find: (provider, id) => (provider === "fake" ? { provider, id } : undefined),
    getAll: () => [],
    complete: async (model, context, options) => {
      complete.push({ model, prompt: context.messages[0].content, options });
      return { stopReason: "stop", content: [{ type: "text", text: `NOTES from ${model.id}` }] };
    },
  },
  ui: {
    confirm: async () => true,
    setStatus: (key, text) => uiCalls.push(["status", key, text]),
    setWidget: (key, lines, options) => uiCalls.push(["widget", key, lines, options]),
    notify: (message, type) => uiCalls.push(["notify", message, type]),
    select: async (title, options) => (uiCalls.push(["select", title, options]), selected),
    custom: (factory) => new Promise((resolve) => (popup = factory(fakeTui, fakeTheme, {}, resolve))),
    ...over,
  },
});
const last = (kind) => uiCalls.filter((c) => c[0] === kind).at(-1);
const uc = uiCtx();

// The footer entry and the widget come from `rkb status`, in the background, after the session starts.
await on("session_start", { reason: "startup" }, uc);
await until(() => last("status"), "status line");
assert.deepEqual(last("status"), ["status", "rkb", "rkb: 12 inbox (3 high) | 1 request"]);
// Injected lessons show above the editor until the next prompt.
await until(() => last("widget")?.[2], "widget lines");
assert.deepEqual(last("widget").slice(1), ["rkb", ["rkb prompt: Stale build cache (2b00000034)"], { placement: "aboveEditor" }]);
await on("input", { text: "next", source: "interactive" }, uc);
assert.equal(last("widget")[2], undefined, "cleared at the next prompt");
await quiet();
assert.equal(last("widget")[2], undefined, "an id shows once");
// No UI, no calls; a harness without a method skips it.
const calls = uiCalls.length;
await on("session_start", { reason: "startup" }, ctx(false));
await on("input", { text: "x", source: "interactive" }, ctx(false));
const bare = uiCtx({ setStatus: undefined, setWidget: undefined, notify: undefined });
await on("session_start", { reason: "startup" }, bare);
await quiet();
assert.equal(uiCalls.length, calls);
// rkb that cannot run clears the entry.
const keep = process.env.PATH;
process.env.PATH = "/nonexistent";
await on("tool_result", { ...bash("rkb search x"), content: [], isError: false }, uc);
await until(() => last("status")[2] === undefined, "cleared status");
process.env.PATH = keep;

// The popup renders each view to lines no wider than the terminal.
const render = (width = 60) => {
  const lines = popup.render(width);
  for (const l of lines) assert.ok(visibleWidth(l) <= width, `too wide: ${l}`);
  return lines.join("\n").replace(/\x1b_[^\x07]*\x07|\x1b\[[0-9;]*m/g, "");
};
const key = async (data, cond = () => true) => {
  popup.handleInput(data);
  await until(cond, `after ${JSON.stringify(data)}`);
};
const open = async (c = uc) => {
  popup = undefined;
  const done = commands["rkb-status"].handler("", c);
  await until(() => popup, "popup");
  return done;
};
const opened = open();
await until(() => popup, "popup");
let view = render();
assert.match(view, /rkb\s+12 inbox \| 3 high \| 0 curate/);
assert.match(view, /\[Search\]\s+Inbox 12\s+Requests 1/);
assert.match(view, /Search lessons/);
popup.handleInput("s");
for (const c of "linker") popup.handleInput(c);
assert.match(render(), /linker/);
await key("\r", () => /1 result for "linker"/.test(render()));
view = render();
assert.match(view, /\[PITFALL\] Linker undefined reference\s+1a00000012/);
assert.match(view, /#########- 0\.92 \| applies likely/);
assert.match(view, /Link the library after the objects\./);
assert.ok(log().includes("search linker --format json"));
await key("\r", () => /Put -lfoo after main\.o/.test(render()));
view = render();
assert.match(view, /\[PITFALL\] Linker undefined reference/);
assert.match(view, /active \| applies likely \| general\/cmake\/linker\.md/);
assert.match(view, /w Worked \| x\s+Irrelevant/);
await key("w", () => log().includes("used --worked 1a00000012 --session s-"));
await until(() => /Recorded 1a00000012 as worked/.test(render()), "worked notice");
await key("b");
assert.match(render(), /1 result for "linker"/);
// Inbox: grouped by priority with verdicts, then an item with its body cut and the rkb command named.
await key("i", () => /make fails/.test(render()));
view = render();
assert.ok(view.indexOf("* high 1") < view.indexOf("make fails") && view.indexOf("make fails") < view.indexOf("* low 1"), view);
assert.match(view, /make fails with a stale cache\s+1 keep \| 2 min/);
assert.match(view, /observed\s+prefer ninja over make\s+1 h/);
assert.match(view, /g Triage \| d Distill \| c Curate 0 \| t Retro/);
await key("\r", () => /line 0 of a long/.test(render()));
view = render(50);
assert.match(view, /\[NOTE\] make fails with a stale cache\s+cd236c6b0b/);
assert.match(view, /high priority \| 2 min \| \/work\/proj/);
assert.match(view.replace(/\n/g, " "), /\.\.\. \d+ more lines\. Run `rkb inbox show cd236c6b0b` to read all\./);
assert.ok(!view.includes("line 79 "), "the long body is cut");
// A 300-line item fits the 30-row terminal minus pi's own chrome, shows the note and scrolls.
canned("inbox-show", { body: Array.from({ length: 300 }, (_, i) => `row ${i} of a very long extract`).join("\n"), meta: {} });
await key("b");
await key("\r", () => /row 0 of/.test(render()));
view = render(60);
const budget = fakeTui.terminal.rows - 8;
assert.ok(view.split("\n").length <= budget, `${view.split("\n").length} lines > ${budget}`);
assert.match(view, /lines 1-\d+ of 300/);
assert.match(view.replace(/\n/g, " "), /more lines\. Run `rkb inbox show cd236c6b0b` to read all\./);
assert.match(view, /Space\/u page \| g\/G top\/bottom/);
await key(" ");
view = render(60);
assert.ok(!view.includes("row 0 of") && /row 10 of/.test(view) && view.split("\n").length <= budget, view);
await key("G");
view = render(60);
assert.match(view, /row 299 of/);
assert.ok(!/more lines/.test(view), "no note at the end");
await key("u");
assert.ok(/row 2\d\d of/.test(render(60)) && !/row 299 of/.test(render(60)));
await key("g");
assert.match(render(60), /row 0 of/);
await key("\x1b[6~");
assert.ok(!render(60).includes("row 0 of"), "PgDn still works where passed");
canned("inbox-show", { body: longBody, meta: { cwd: "/work/proj" } });
await key("b");
assert.match(render(), /\* high 1/);
await key("\x1b[B");
assert.match(render(), /> observed\s+prefer ninja/);
// Triage runs rkb and redraws the inbox with a toast.
await key("g", () => /Triaged: 1 keep, 0 known, 1 unsure, 0 drop/.test(render()));
assert.ok(uiCalls.some((c) => c[0] === "notify" && /triaged 2 inbox items/.test(c[1])));
assert.ok(log().includes("inbox triage --format json"));
// Requests view lists the open request.
await key("r");
view = render();
assert.match(view, /> needs you\s+r-1a2b/);
assert.match(view, /Install the rkb plugin/);
assert.match(view, /yes \/ no/);
// Esc closes.
popup.handleInput("\x1b");
await opened;
// Distill from the inbox closes the popup and sends the prompt.
const second = open();
await until(() => popup, "popup again");
sent.length = 0;
popup.handleInput("i");
await until(() => /make fails/.test(render()), "inbox again");
popup.handleInput("d");
await second;
assert.match(sent.at(-1), /^Distill the rkb inbox into lessons/);

// A request is answered through `select` and `rkb confirm`; a dismissed dialog leaves it open.
canned("confirm", { id: "done" });
const third = open();
await until(() => popup, "popup 3");
popup.handleInput("r");
const shownBefore = uiCalls.length;
const prev = popup;
popup.handleInput("\r");
await until(() => last("select"), "select");
assert.deepEqual(last("select").slice(1), ["Install the rkb plugin?", ["yes", "no"]]);
await until(() => log().includes("confirm r-1a2b --choice yes --format json"), "confirm");
await until(() => popup !== prev, "popup reopened on requests");
assert.match(render(), /> needs you\s+r-1a2b/);
assert.ok(uiCalls.slice(shownBefore).some((c) => c[0] === "notify" && c[1] === "rkb: r-1a2b: yes"));
selected = undefined;
const before2 = log().split("\n").filter((l) => l.startsWith("confirm")).length;
const again = popup;
popup.handleInput("\r");
await until(() => popup !== again, "reopened after a dismissed dialog");
popup.handleInput("\x1b");
await third;
assert.equal(log().split("\n").filter((l) => l.startsWith("confirm")).length, before2, "dismissed: no confirm");
// A failed confirm shows a toast.
selected = "no";
rmSync(`${stub}/confirm.json`);
const fourth = open();
await until(() => popup, "popup 4");
popup.handleInput("r");
const p4 = popup;
popup.handleInput("\r");
await until(() => popup !== p4, "reopened after a failed confirm");
assert.ok(uiCalls.some((c) => c[0] === "notify" && c[2] === "error" && /rkb confirm r-1a2b failed/.test(c[1])));
popup.handleInput("\x1b");
await fourth;
selected = "yes";

// Without a popup, /rkb-status shows the `rkb status` text: a notice with a UI that lacks custom, stdout without one.
canned("status", "inbox: 12 items\n");
const text = () => uiCalls.filter((c) => c[0] === "notify").at(-1)[1];
await commands["rkb-status"].handler("", uiCtx({ custom: undefined }));
assert.equal(text(), "inbox: 12 items");
const lines = [];
const out = console.log;
console.log = (m) => lines.push(m);
await commands["rkb-status"].handler("", ctx(false));
console.log = out;
assert.deepEqual(lines, ["inbox: 12 items"]);
canned("status", {
  inbox: { total: 12, priority: { 1: 8, 2: 1, 3: 3 } },
  curate: 0,
  requests: [],
  injected: [],
});

// Observation after a settled turn (pi): prepare, the registry's model, finish, then triage.
if (harness === "omp") assert.ok(!handlers.agent_settled, "omp has no agent_settled");
else {
  const job = { job: "j1", part: 0, prompt: "OBSERVE THIS", models: [{ entry: "session", model: "fake/m1" }] };
  canned("job-prepare-observe", job);
  canned("job-finish-j1", { outcome: "added", notes: 2 });
  uiCalls.length = 0;
  await on("agent_settled", { type: "agent_settled" }, uc);
  await until(() => uiCalls.some((c) => c[0] === "notify" && c[1] === "rkb: 2 notes for the inbox"), "notes toast");
  assert.deepEqual([complete.at(-1).model.id, complete.at(-1).prompt, complete.at(-1).options.maxTokens], ["m1", "OBSERVE THIS", 4096]);
  const l = log();
  assert.match(l, /job prepare observe --session s-pi --transcript .*pi\.jsonl --harness pi --cwd .* --model fake\/m1 --format json/);
  assert.ok(l.includes("job finish j1 --part 0 --model session --format json <<< NOTES from m1"), l);
  await until(() => log().includes("job prepare triage --harness pi --model fake/m1"), "triage job");
  assert.ok(log().indexOf("inbox triage") < log().indexOf("job prepare triage"));
  // No model for any chain entry: the job finishes with --failed.
  canned("job-prepare-observe", { ...job, models: [{ entry: "gone", model: "nope/x" }, { entry: "session", model: "fake/none" }] });
  await on("agent_settled", { type: "agent_settled" }, { ...uc, modelRegistry: { find: () => undefined, getAll: () => [] } });
  await until(() => log().includes("--failed gone: no model nope/x; session: no model fake/none"), "failed job");
  // The observer's own session observes nothing.
  await until(() => (log().match(/job prepare triage/g) ?? []).length >= 2, "second triage job");
  const jobs = () => log().split("\n").filter((l) => /^(job|inbox triage)/.test(l)).length;
  const jobs0 = jobs();
  process.env.RKB_OBSERVER = "1";
  await on("agent_settled", { type: "agent_settled" }, uc);
  await quiet();
  delete process.env.RKB_OBSERVER;
  assert.equal(jobs(), jobs0, "RKB_OBSERVER observes nothing");
}
process.env.PATH = uiPath;

console.log(`ok ${harness}`);

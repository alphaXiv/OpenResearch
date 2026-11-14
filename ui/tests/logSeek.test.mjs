import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { createElement } from "react";
import { DetailDrawer } from "../src/components/DetailDrawer.tsx";
import { LogTerminal } from "../src/components/LogTerminal.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, render, serve, settle } from "./render.mjs";

const { evidence } = JSON.parse(readFileSync(new URL("./fixtures/runMonitor/flagged.json", import.meta.url), "utf8")).flag;
const filler = (n, bytes) => Array.from({ length: n }, (_, i) => `step ${i + 1} ok`.padEnd(bytes - 1, ".")).join("\n") + "\n";
// A short log whose line 91 starts at the evidence's first byte.
const width = Math.floor(evidence.byteFrom / (evidence.from - 1));
const short = filler(evidence.from - 2, width);
const before = short + `step ${evidence.from - 1} ok`.padEnd(evidence.byteFrom - short.length - 1, ".") + "\n";
const log = before + evidence.lines.join("\n") + "\n" + filler(40, 40);
assert.equal(before.length, evidence.byteFrom);

/** Serves run r1's log as file `ident`; a read naming another file is told it was replaced. */
function serveLog(ident = evidence.ident) {
  serve((path) => {
    if (path === "/api/compute/hosts") return [];
    const read = path.match(/^\/api\/runs\/r1\/log\?(.*)$/);
    if (!read) return undefined;
    const query = new URLSearchParams(read[1]);
    if (query.has("ident") && query.get("ident") !== ident) return { kind: "reset", info: { ident, len: log.length } };
    const start = Number(query.get("start"));
    const end = Math.min(Number(query.get("end")), log.length);
    return { kind: "data", ident, start, data: Buffer.from(log.slice(start, end)).toString("base64"), eof: end >= log.length, len: log.length, kept: [[0, log.length]] };
  });
}

/** The lines the terminal shows, top first, once they contain `text`. */
async function shown(container, text) {
  for (let tries = 0; tries < 100; tries++) {
    const rows = [...container.querySelectorAll(".xterm-rows > div")].map((row) => row.textContent.trimEnd());
    if (rows.some((row) => row.includes(text))) return rows.filter(Boolean);
    await new Promise((resolve) => setImmediate(resolve));
    await settle();
  }
  assert.fail(`the terminal never showed ${text}`);
}

const run = { id: "r1", experimentId: "e1", createdAt: 1, status: "running", contact: null, host: null, backend: null };
const drawer = (props) => createElement(DetailDrawer, { experiment: { id: "e1", projectId: "p", slug: "lr" }, project: {}, view: "terminal", runs: [run], selectedRunId: "r1", onSelectRun() {}, ...props });

test("opening evidence on a short log that is already open shows the cited lines at the top, and only that viewer jumps", async () => {
  serveLog();
  const taken = [];
  const { container, rerender } = await render(drawer({}));
  assert.ok((await shown(container, "step 40 ok")).at(-1).startsWith("step 40 ok"), "the open log follows its end");

  const seek = { runId: "r1", at: evidence.byteFrom, ident: evidence.ident };
  await rerender(drawer({ logSeek: seek, onLogSeekTaken: (value) => taken.push(value) }));
  const rows = await shown(container, "::orx::");
  assert.equal(rows[0], evidence.lines[0], "the cited line is the first the user sees");
  assert.deepEqual(taken, [seek], "taken once");
  assert.ok(find("button", m.log_follow()), "the log offers to follow again");

  const other = await render(createElement(LogTerminal, { run }));
  assert.ok((await shown(other.container, "step 40 ok")).at(-1).startsWith("step 40 ok"), "a viewer the seek wasn't given follows the end");
});

test("evidence from a log replaced since it was cited opens the new log at its end and says why", async () => {
  serveLog("2-200");
  const { container } = await render(drawer({ logSeek: { runId: "r1", at: evidence.byteFrom, ident: evidence.ident } }));
  await shown(container, "step 40 ok");
  assert.ok(find('[role="status"]', m.log_replaced_since_cited()));
});

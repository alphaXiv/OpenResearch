import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { act, createElement } from "react";
import { Watch } from "../src/components/run/Watch.tsx";
import { m } from "../src/paraglide/messages.js";
import { journalRows } from "../src/runMonitor/journal.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { render, serveApp, settle } from "./render.mjs";

const running = JSON.parse(readFileSync(new URL("./fixtures/runMonitor/running.json", import.meta.url), "utf8"));
const now = running.asOf;
const text = (element) => element.textContent.replace(/[⁦-⁩]/g, "");
const look = (id, at, template, extra = {}) => ({ id, at, kind: "ceiling", whole: 0.1, said: "new", text: template, template, bindings: { loss: 2.6 }, with: [], why: "", ...extra });
const monitor = (watch) => ({ ...running, watch: { state: "on", model: "Claude Haiku", looks: [], spentUsd: null, nextAt: null, floorUntil: null, error: null, ...watch } });

test("the journal lists changes, not looks: words that held fold into one row that says since when", () => {
  const looks = [
    look("1", now - 50_000, "Loss **{loss}**, falling."),
    look("2", now - 40_000, "Loss **{loss}**, falling.", { said: "same" }),
    look("3", now - 30_000, "Loss **{loss}**, falling.", { said: "same" }),
    look("4", now - 20_000, "Loss went to **{loss}**.", { said: "flag" }),
    look("5", now - 10_000, null, { said: "error", text: "Claude Code is signed out", whole: null }),
  ];
  const rows = journalRows(looks);
  assert.deepEqual(rows.map((row) => [row.look.id, row.count]), [["5", 1], ["4", 1], ["3", 3]]);
  assert.equal(rows[2].since.id, "1");
});

async function watched(watch, routes = {}) {
  serveApp(routes);
  const view = await render(createElement(Watch, { monitor: monitor(watch), runId: running.runId, tone: "running", live: true, now }), createWorkspace("test"));
  await settle();
  return view;
}

test("the Watch section says what was said and when, what the looks cost, and Look now asks for one", async () => {
  const looks = [
    look("1", now - 50_000, "Loss **{loss}**, falling."),
    look("2", now - 40_000, "Loss **{loss}**, falling.", { said: "same", with: ["Depth 6"] }),
  ];
  let asked = 0;
  const { container } = await watched({ looks, spentUsd: 0.0021 }, { [`POST /api/runs/${running.runId}/look`]: () => { asked += 1; return { lookId: "3", state: "prepared" }; } });
  assert.match(text(container), /Loss 2\.60, falling\./);
  assert.match(text(container), /The same on 2 looks since/);
  assert.match(text(container), /Looked at with Depth 6/);
  assert.match(text(container), /2 looks · Claude Haiku · \$0\.0021/);
  const button = [...container.querySelectorAll("button")].find((node) => text(node) === m.run_watch_look_now());
  await act(async () => button.click());
  await settle();
  assert.equal(asked, 1);
});

test("Look now waits out the least gap between looks, and a refusal says why where it was pressed", async () => {
  const soon = await watched({ looks: [look("1", now - 5_000, "Loss **{loss}**.")], floorUntil: now + 60_000 });
  const waiting = [...soon.container.querySelectorAll("button")].find((node) => text(node) === m.run_watch_look_now());
  assert.equal(waiting.getAttribute("aria-disabled"), "true");
  await soon.unmount();
  const { container } = await watched({ looks: [] }, {
    [`POST /api/runs/${running.runId}/look`]: () => Response.json({ error: "This chat's agent has no watching model, so its runs are not watched." }, { status: 409 }),
  });
  const button = [...container.querySelectorAll("button")].find((node) => text(node) === m.run_watch_look_now());
  await act(async () => button.click());
  await settle();
  assert.match(text(container.querySelector('[role="status"]')), /no watching model/);
});

test("a run nobody watches says why in place of a journal", async () => {
  for (const [watch, said] of [
    [{ state: "needs_pick", model: null }, m.run_watch_needs_pick()],
    [{ state: "off", model: null }, m.run_watch_off()],
    [{ state: "no_model", model: null, why: "Codex offers no GPT Luna to watch with" }, "Not watched: Codex offers no GPT Luna to watch with."],
  ]) {
    const { container, unmount } = await watched(watch);
    assert.equal(text(container.querySelector("p")), said);
    assert.equal([...container.querySelectorAll("button")].length, 0, "nothing to look with");
    await unmount();
  }
});

test("the journal says who settled a flag and how, under the look that raised it, and orx's own flags stand as rows", async () => {
  const looks = [look("1", now - 50_000, "Loss **{loss}**, falling."), look("2", now - 30_000, "Loss jumped to **{loss}**.", { said: "flag" })];
  const flag = (extra) => ({ at: now - 30_000, whole: 0.2, template: "x", detail: "x", evidence: null, mode: "copilot", ...extra });
  serveApp({});
  const view = await render(createElement(Watch, {
    monitor: {
      ...monitor({ looks }),
      flags: [
        flag({ id: "f1", source: "look", lookId: "2", state: "kept", choice: { by: "user", c: "keep", at: now - 20_000, reason: "Warm-up noise." } }),
        flag({ id: "f2", source: "tripwire", at: now - 10_000, detail: "No progress for **2m** since step 300.", state: "stopped", choice: { by: "agent", c: "stop", at: now - 5_000, reason: null } }),
      ],
    },
    runId: running.runId, tone: "running", live: true, now,
  }), createWorkspace("test"));
  await settle();
  const rows = [...view.container.querySelectorAll("li")].map(text);
  assert.match(rows[0], new RegExp(`${m.run_watch_flagged_orx()}.*No progress for 2m since step 300\\.`));
  assert.ok(rows[0].includes(m.run_watch_stopped_by({ agent: "The agent" })), rows[0]);
  assert.match(rows[1], new RegExp(`${m.run_watch_flagged()}.*Loss jumped to 2\\.60\\.`));
  assert.ok(rows[1].includes(`${m.run_watch_kept()} Warm-up noise.`), rows[1]);
  await view.unmount();
});

test("a look that left out a credential says where, once, on its own row", async () => {
  const note = "The watcher was not shown line 12 of the log: it holds what looks like a Hugging Face token. Replace that credential, and stop the run printing it.";
  const looks = [
    look("1", now - 30_000, "Loss **{loss}**, falling.", { withheld: [note] }),
    look("2", now - 20_000, "Loss **{loss}**, falling.", { said: "same", withheld: [] }),
  ];
  assert.deepEqual(journalRows(looks).map((row) => row.look.id), ["2", "1"], "the told row keeps its own place");
  const { container } = await watched({ looks });
  assert.equal(text(container).split("line 12 of the log").length, 2, text(container));
});

test("a look that flagged while a flag was open says it was noted under that flag", async () => {
  const looks = [look("1", now - 20_000, "Checkpoint write failed at step **450**.", { notedUnder: now - 60_000 })];
  const { container } = await watched({ looks });
  assert.match(text(container), /Noted while the flag from .+ was open\./);
  assert.doesNotMatch(text(container), new RegExp(m.run_watch_flagged()));
});

test("a look says what it left out to keep within its budget", async () => {
  const cut = "To keep this look within 4096 tokens it left out 140 new log lines; its earlier sentences.";
  const looks = [look("1", now - 20_000, "Loss **{loss}**, falling.", { omitted: cut })];
  const { container } = await watched({ looks });
  assert.ok(text(container).includes(cut), text(container));
});

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { act, createElement } from "react";
import { Watch } from "../src/components/run/Watch.tsx";
import { clockText } from "../src/runMonitor/words.ts";
import { m } from "../src/paraglide/messages.js";
import { journalRows } from "../src/runMonitor/journal.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { render, serveApp, settle } from "./render.mjs";

const running = JSON.parse(readFileSync(new URL("./fixtures/runMonitor/running.json", import.meta.url), "utf8"));
const now = running.asOf;
const text = (element) => element.textContent.replace(/[⁦-⁩]/g, "");
const clock = (at) => clockText(at).replace(/[⁦-⁩]/g, "");
const look = (id, at, template, extra = {}) => ({ id, at, kind: "ceiling", whole: 0.1, said: "new", text: template, template, bindings: { loss: 2.6 }, with: [], why: "", stage: null, withheld: [], ...extra });
const monitor = (watch) => ({ ...running, watch: { state: "on", model: "Claude Haiku", looks: [], spentUsd: null, floorUntil: null, error: null, ...watch } });

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

/** Opens the journal entry whose line matches `line`, or the `line`th, as a click or Enter on it does. */
async function expand(container, line) {
  const entries = [...container.querySelectorAll("li > button[aria-expanded]")];
  const entry = typeof line === "number" ? entries[line] : entries.find((button) => line.test(text(button)));
  assert.ok(entry, `an entry reads ${line}`);
  assert.equal(entry.getAttribute("aria-expanded"), "false");
  await act(async () => entry.click());
  await settle();
  assert.equal(entry.getAttribute("aria-expanded"), "true");
  return entry.closest("li");
}

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
  assert.doesNotMatch(text(container), /The same on 2 looks since|Looked at with/, "how a row came about waits behind it");
  await expand(container, /Loss 2\.60/);
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
  const flag = (extra) => ({ at: now - 30_000, whole: 0.2, template: "x", detail: "x", evidence: null, ...extra });
  serveApp({});
  const view = await render(createElement(Watch, {
    monitor: {
      ...monitor({ looks }),
      flags: [
        flag({ id: "f1", source: "look", lookId: "2", state: "kept", choice: { by: "user", c: "keep", at: now - 20_000, reason: "Warm-up noise." } }),
        flag({ id: "f2", source: "tripwire", at: now - 10_000, detail: "No progress for **2m** since step 300.", state: "stopped", choice: { by: "agent", c: "stop", at: now - 5_000, reason: null } }),
        flag({ id: "f3", source: "tripwire", at: now - 12_000, detail: "Loss became **NaN**.", state: "stopped", choice: { by: "rule", c: "stop", at: now - 11_000, reason: null, rule: "diverging", moot: true } }),
      ],
    },
    runId: running.runId, agent: null, tone: "running", live: true, now,
  }), createWorkspace("test"));
  await settle();
  const rows = () => [...view.container.querySelectorAll("li")].map(text);
  assert.equal(rows()[0], `${clock(now - 10_000)}No progress for 2m since step 300.`, "a flag says what it found, and no more");
  assert.equal([...view.container.querySelectorAll('li [role="img"]')].map((icon) => icon.getAttribute("aria-label")).join(), [m.run_watch_flagged(), m.run_watch_flagged(), m.run_watch_flagged()].join());
  for (const line of [/No progress/, /Loss became NaN/, /Loss jumped/]) await expand(view.container, line);
  assert.ok(rows()[0].includes(m.run_watch_stopped_by({ agent: "The agent" })), rows()[0]);
  assert.match(rows()[1], /Loss became NaN\./);
  assert.ok(rows()[1].includes(m.run_watch_stopped_by_rule({ name: "diverging" })), "a rule's stop settles the flag it never chose on");
  assert.equal([...view.container.querySelectorAll("button")].filter((button) => /stop|keep/i.test(text(button))).length, 0, "nothing left to choose");
  assert.match(rows()[2], /Loss jumped to 2\.60\./);
  assert.ok(rows()[2].includes(`${m.run_watch_kept()} Warm-up noise.`), rows()[2]);
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
  const warning = container.querySelector('li [role="img"]');
  assert.match(warning.getAttribute("aria-label"), /line 12 of the log/);
  assert.doesNotMatch(text(container), /line 12 of the log/);
  await expand(container, 1);
  assert.equal(text(container).split("line 12 of the log").length, 2, text(container));
});

test("a look that flagged while a flag was open says it was noted under that flag", async () => {
  const looks = [look("1", now - 20_000, "Checkpoint write failed at step **450**.", { notedUnder: now - 60_000 })];
  const { container } = await watched({ looks });
  await expand(container, /Checkpoint write failed/);
  assert.match(text(container), /Noted while the flag from .+ was open\./);
  assert.doesNotMatch(text(container), new RegExp(m.run_watch_flagged()));
});

test("a look says what it left out to keep within its budget only when its entry is opened", async () => {
  const cut = "To keep this look within 4096 tokens it left out 140 new log lines; its earlier sentences.";
  const looks = [look("1", now - 20_000, "Loss **{loss}**, falling.", { omitted: cut })];
  const { container } = await watched({ looks });
  assert.ok(!text(container).includes(cut), text(container));
  await expand(container, /Loss 2\.60/);
  assert.ok(text(container).includes(cut), text(container));
});

test("a failed look is one muted line whose reason and bookkeeping wait behind it, and a silent look is no line", async () => {
  const why = "The watcher's reply was not accepted: r1: {dev_loss.vs} compares with the parent, which r1 has no comparison for";
  const cut = "To keep this look within 4096 tokens it left out 148 new log lines.";
  const looks = [
    look("1", now - 50_000, "Loss **{loss}**, falling."),
    look("2", now - 40_000, null, { text: "", whole: null }),
    look("3", now - 30_000, null, { said: "error", text: why, whole: null, omitted: cut, with: ["Depth 6"] }),
    look("4", now - 20_000, null, { said: "interrupted", text: "The watcher stopped before this look finished.", whole: null }),
  ];
  const { container } = await watched({ looks });
  const rows = [...container.querySelectorAll("li")];
  assert.deepEqual(rows.map(text), [
    `${clock(now - 20_000)}${m.run_watch_look_failed()}`,
    `${clock(now - 30_000)}${m.run_watch_look_failed()}`,
    `${clock(now - 50_000)}Loss 2.60, falling.`,
  ]);
  assert.match(rows[1].querySelector("button > span:last-child").className, /text-ink-subtext/, "a failure reads muted");
  assert.match(rows[1].querySelector("button > span:last-child").className, /truncate/);
  const failed = await expand(container, 1);
  for (const said of [why, cut, "Looked at with Depth 6"]) assert.ok(text(failed).includes(said), text(failed));
  const cutShort = await expand(container, 0);
  assert.ok(text(cutShort).includes(m.run_watch_interrupted()), text(cutShort));
});

test("a stale Look now button uses the wait returned by the workspace", async () => {
  const notBefore = now + 330_000;
  const { container } = await watched({ looks: [] }, {
    [`POST /api/runs/${running.runId}/look`]: () => Response.json({ error: "A look just ran.", notBefore }, { status: 409 }),
  });
  const button = () => [...container.querySelectorAll("button")].find((node) => text(node) === m.run_watch_look_now());
  await act(async () => button().click());
  await settle();
  assert.equal(button().getAttribute("aria-disabled"), "true");
  assert.equal(text(container.querySelector('[role="status"]')), m.run_watch_look_too_soon({ clock: clockText(notBefore) }).replace(/[⁦-⁩]/g, ""));
});

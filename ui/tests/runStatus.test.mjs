import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { act, createElement } from "react";
import { RunStatus } from "../src/components/run/RunStatus.tsx";
import { autoDir } from "../src/i18n.ts";
import { setLocale } from "../src/locale.ts";
import { m } from "../src/paraglide/messages.js";
import { runModel } from "../src/runMonitor/model.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { find, press, render, serve, settle } from "./render.mjs";

const base = JSON.parse(readFileSync(new URL("./fixtures/runMonitor/pipeline.json", import.meta.url), "utf8"));
const now = Date.now();
const bare = (text) => text.replace(/[⁦-⁩]/g, "");

async function status(answer, extra = {}) {
  const watcher = "The held-out score keeps improving. " + "The remaining checks cover the run's question. ".repeat(15);
  const data = { ...base, ...extra, sentence: { template: watcher, t: now } };
  const run = { id: data.runId, projectId: "p", experimentId: "e", status: "running", createdAt: now - 60_000, startedAt: now - 60_000, updatedAt: now, contact: null, cancelling: false, host: "lab-a40", backend: { kind: "slurm_job" }, monitor: { rev: data.rev, generation: data.generation, asOf: data.asOf } };
  const model = runModel({ item: { sessionId: "s", experimentName: "Distil to six layers", run }, monitor: data, shown: true, agent: "Claude", now, nextRetryMs: null });
  serve(() => answer ? answer(data) : data);
  const workspace = createWorkspace("test");
  workspace.queryClient.setDefaultOptions({ queries: { retry: false } });
  const opened = [];
  const view = await render(createElement(RunStatus, { model, onOpenRunPage: (...args) => opened.push(args) }), workspace);
  const trigger = view.container.querySelector("button");
  assert.ok(trigger.getAttribute("aria-label").includes(model.name));
  await act(async () => trigger.click());
  await settle();
  return { ...view, trigger, watcher, run, opened };
}

test("the status card keeps the full watcher sentence, machine and linked run name", async () => {
  const { watcher, run, opened } = await status();
  const card = document.querySelector('[role="dialog"]');
  assert.ok(card);
  assert.ok(bare(card.textContent).includes(watcher));
  assert.ok(card.textContent.includes("lab-a40"));
  assert.equal(card.querySelector('[aria-current="step"]').textContent.includes("train"), true);
  assert.deepEqual([...card.querySelectorAll("ol > li")].map((li) => bare(li.querySelector("span").childNodes[0].textContent)), ["Setup", "train", "eval", "export"]);
  await act(async () => card.querySelector('[role="link"]').click());
  assert.equal(opened.length, 1);
  assert.equal(opened[0][0], run);
});

test("Escape closes the card and returns focus to its status", async () => {
  const { trigger } = await status();
  await act(async () => find('[role="dialog"] [role="link"]').focus());
  const event = await press("Escape");
  await settle();
  assert.ok(event.defaultPrevented);
  assert.equal(trigger.getAttribute("aria-expanded"), "false");
  assert.equal(document.activeElement, trigger);
});

test("watcher prose keeps its own direction in an RTL status and card", async () => {
  await setLocale("ar");
  try {
    const { watcher, trigger } = await status();
    assert.ok(trigger.textContent.includes(autoDir(watcher)));
    assert.ok(find('[role="dialog"]').textContent.includes(autoDir(watcher)));
  } finally {
    await setLocale("en");
  }
});

test("a failed stage read offers Retry without claiming that no stages exist", async () => {
  let fail = true;
  await status((data) => fail ? new Response(JSON.stringify({ error: "The run could not be read." }), { status: 500 }) : data);
  assert.ok(find('[role="alert"]', m.run_monitor_load_failed()));
  fail = false;
  await act(async () => find('[role="alert"] button', m.app_retry()).click());
  await settle();
  assert.equal(find('[role="alert"]'), null);
  assert.ok(find('[role="dialog"] ol'));
});

test("an unfinished stage read says it is loading", async () => {
  let release;
  await status((data) => new Promise((resolve) => { release = () => resolve(data); }));
  assert.ok(find('[role="status"]', m.common_loading()));
  assert.equal(find('[role="dialog"] ol'), null);
  await act(async () => release());
  await settle();
  assert.ok(find('[role="dialog"] ol'));
});


test("the whole-run line shows a floor and names stages with no timing", async () => {
  await status(null, { estimate: { loMs: 3_840_000, midMs: 3_900_000, hiMs: 4_000_000, at: now, pending: false, pastLimitMs: null, floor: true, untimed: ["sweep"] } });
  const card = find('[role="dialog"]');
  assert.match(bare(card.textContent), /Whole run: At least 1h 4m left.*sweep not timed/);
});

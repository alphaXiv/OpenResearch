import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { codex, renderChat, session } from "./chat.mjs";
import { model } from "./models.mjs";
import { press, settle } from "./render.mjs";

const free = (id, name) => ({ ...model(id, name), free: true });
const opencode = {
  ...codex, id: "opencode", name: "OpenCode", defaultModel: "opencode/big-pickle",
  models: [
    model("opencode/big-pickle", "Big Pickle"),
    free("opencode/glm-5.3-flash-free", "GLM 5.3 Flash"),
    free("opencode/inkling-2-small-free", "Inkling 2 Small"),
    free("opencode/deepseek-v4.1-flash-free", "DeepSeek V4.1 Flash"),
  ],
};

/** `days` ago where the user is, as the feed dates releases. */
const daysAgo = (days) => {
  const day = new Date();
  day.setDate(day.getDate() - days);
  return [day.getFullYear(), day.getMonth() + 1, day.getDate()].map((part) => String(part).padStart(2, "0")).join("-");
};

const nudge = (fields = {}) => ({ id: "nudge-1", model: "opencode/deepseek-v4.1-flash-free", kind: "new", colors: { from: "#3b82f6", to: "#14b8a6" }, ...fields });

const feed = (fields = {}) => ({
  featured: [{ model: "opencode/deepseek-v4.1-flash-free" }, { model: "opencode/not-offered-free" }, { model: "opencode/inkling-2-small-free" }],
  nudge: nudge(),
  ...fields,
});

/** The open chat over a server whose feed is `served` and whose saved preferences, which the page's changes update, start as `state`. */
async function open(served = feed(), state = {}) {
  const saved = { onboardingCompleted: true, opencodeFreeModels: true, seenNudges: [], ...state };
  const chat = await renderChat("/projects/p/tasks/s", {
    sessions: [session("s")],
    harnesses: [codex, opencode],
    routes: {
      "GET /api/model-feed": () => served,
      "GET /api/settings/ui-state": () => saved,
      "POST /api/settings/ui-state": (init) => Object.assign(saved, JSON.parse(init.body)),
    },
  });
  return {
    ...chat,
    saved,
    seen: () => chat.requests.filter(({ request, body }) => request === "POST /api/settings/ui-state" && body.seenNudges).map(({ body }) => body.seenNudges),
    events: () => chat.requests.filter(({ request }) => request === "POST /api/telemetry/event").map(({ body }) => body),
  };
}

const trigger = () => document.querySelector('[data-onboarding="model-picker"] button');
/** The button over the model button pointing at `name`, outside the picker. */
const callout = (name) => [...document.querySelectorAll("button")].find((button) =>
  button.textContent.startsWith(name) && button !== trigger() && !button.closest(".model-menu")) ?? null;
const shownNudge = () => callout("DeepSeek V4.1 Flash");
const sections = () => [...document.querySelectorAll("[data-section]")].map((section) => section.dataset.section);
const rows = (section) => [...document.querySelectorAll(`[data-section="${section}"] [data-row] [data-nav]`)].map((row) => row.textContent);

async function openPicker() {
  await act(async () => trigger().click());
  await settle();
}

test("Free lists two free models, the feed's featured ones first, then the rest on request, and nowhere else", async () => {
  await open(feed({ nudge: null }));
  await openPicker();

  assert.deepEqual(sections().slice(0, 1), ["free"]);
  assert.deepEqual(rows("free"), ["DeepSeek V4.1 Flash", "Inkling 2 Small"]);
  const more = [...document.querySelectorAll('[data-section="free"] button')].find((button) => button.textContent === m.model_picker_more_free({ count: "1" }));
  await act(async () => more.click());
  assert.deepEqual(rows("free"), ["DeepSeek V4.1 Flash", "Inkling 2 Small", "GLM 5.3 Flash"]);
  const elsewhere = sections().filter((section) => section !== "free").flatMap(rows);
  assert.ok(elsewhere.includes("Big Pickle"));
  assert.ok(!elsewhere.some((row) => row.includes("Flash") || row.includes("Inkling")));
});

test("models released this week are badged New; the newest from a frontier lab is pointed at once, ahead of the free nudge, and the picker opens on it", async () => {
  const released = feed({ releases: { "gpt-6.1-sol": daysAgo(10), "gpt-6-astra": daysAgo(2), "opencode/big-pickle": daysAgo(0) } });
  const chat = await open(released);
  assert.equal(callout("Big Pickle"), null);
  assert.equal(callout("GPT-6 Astra")?.textContent, `GPT-6 Astra${m.model_picker_released_this_week({ lab: "OpenAI" })}`);
  assert.equal(shownNudge(), null);

  await act(async () => callout("GPT-6 Astra").click());
  await settle();
  assert.deepEqual(chat.seen(), [["release:gpt-6-astra"]]);
  assert.deepEqual(chat.events(), ["shown", "opened"].map((action) => ({ name: "model_callout", kind: "release", action, model: "gpt-6-astra", harness: "codex" })));
  assert.ok(!sections().includes("new"));
  assert.equal(document.activeElement.textContent, `GPT-6 Astra${m.model_picker_new()}`);
  const badged = [...document.querySelectorAll("[data-row] [data-nav]")].filter((row) => row.textContent.endsWith(m.model_picker_new()));
  assert.deepEqual(badged.map((row) => row.textContent.replace(m.model_picker_new(), "")), ["GPT-6 Astra", "Big Pickle"]);
  // One callout a launch: the nudge waits for the next.
  await press("Escape", document.body);
  await settle();
  assert.equal(shownNudge(), null);

  await chat.unmount();
  await open(released, chat.saved);
  assert.equal(callout("GPT-6 Astra"), null);
  assert.ok(shownNudge());
});

test("a release is dated today, yesterday or this week", async () => {
  for (const [days, text] of [[0, m.model_picker_released_today], [1, m.model_picker_released_yesterday], [6, m.model_picker_released_this_week]]) {
    const view = await open(feed({ releases: { "gpt-6-astra": daysAgo(days) } }));
    assert.ok(callout("GPT-6 Astra").textContent.endsWith(text({ lab: "OpenAI" })), String(days));
    await view.unmount();
  }
});

test("the nudge shows over the open chat's model button until its model, the free models or its days are gone", async () => {
  const { unmount } = await open();
  assert.equal(shownNudge()?.textContent, `DeepSeek V4.1 Flash${m.model_picker_nudge_new()}`);
  await unmount();

  for (const [served, state] of [
    [feed(), { opencodeFreeModels: false }],
    [feed(), { seenNudges: ["nudge-1"] }],
    [feed({ nudge: nudge({ model: "openresearch-x/research-1" }) }), {}],
    [feed({ nudge: nudge({ kind: "trial", until: "2020-01-31" }) }), {}],
    [feed({ nudge: nudge({ kind: "raffle" }) }), {}],
  ]) {
    const view = await open(served, state);
    assert.equal(shownNudge()?.textContent, undefined, JSON.stringify({ served: served.nudge, state }));
    await view.unmount();
  }
});

test("a trial names its last free day, and a message replaces orx's line", async () => {
  const trial = await open(feed({ nudge: nudge({ kind: "trial", until: "2999-11-30" }) }));
  assert.ok(shownNudge().textContent.endsWith(m.model_picker_nudge_trial_until({ date: "Nov 30" })));
  await trial.unmount();

  await open(feed({ nudge: nudge({ kind: "trial", message: "Ours, free while in preview" }) }));
  assert.ok(shownNudge().textContent.endsWith("Ours, free while in preview"));
});

test("clicking the nudge records it as seen and opens the picker with Free expanded", async () => {
  const chat = await open(feed(), { collapsedModelSections: ["free"] });
  await act(async () => shownNudge().click());
  await settle();

  assert.deepEqual(chat.seen(), [["nudge-1"]]);
  assert.equal(shownNudge()?.textContent, undefined);
  assert.deepEqual(rows("free"), ["DeepSeek V4.1 Flash", "Inkling 2 Small"]);
  assert.equal(document.activeElement.textContent, "DeepSeek V4.1 Flash");
});

test("hovering the model button then leaving it records the nudge as seen, for good", async () => {
  const chat = await open();
  await act(async () => {
    trigger().dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    trigger().dispatchEvent(new MouseEvent("mouseout", { bubbles: true }));
  });
  // The dismissal waits out the gap between the button and the nudge.
  await act(() => new Promise((resolve) => setTimeout(resolve, 250)));
  await settle();

  assert.deepEqual(chat.seen(), [["nudge-1"]]);
  assert.equal(shownNudge()?.textContent, undefined);

  await chat.unmount();
  await open(feed(), chat.saved);
  assert.equal(shownNudge()?.textContent, undefined);
});

test("clicking the model button or pressing Escape records the nudge as seen", async () => {
  const clicked = await open(feed(), { seenNudges: ["older"] });
  await openPicker();
  assert.deepEqual(clicked.seen(), [["older", "nudge-1"]]);
  await clicked.unmount();

  const escaped = await open();
  await press("Escape", document.body);
  await settle();
  assert.deepEqual(escaped.seen(), [["nudge-1"]]);
});

test("a callout never points at the model the chat already runs", async () => {
  await open(feed({ nudge: nudge({ model: "opencode/deepseek-v4.1-flash-free" }), releases: { "gpt-6.1-sol": daysAgo(0) } }));
  assert.ok(shownNudge());
});

test("a pick reports the model, its agent and the section it was picked from", async () => {
  const chat = await open(feed({ nudge: null, releases: { "opencode/big-pickle": daysAgo(1) } }));
  await openPicker();
  await act(async () => [...document.querySelectorAll('[data-section="free"] [data-row] [data-nav]')][1].click());
  await openPicker();
  await act(async () => document.querySelector('[data-row="opencode/big-pickle"] [data-nav]').click());

  assert.deepEqual(chat.events().filter(({ name }) => name === "model_picked"), [
    { name: "model_picked", section: "free", model: "opencode/inkling-2-small-free", harness: "opencode", new: false, free: true },
    { name: "model_picked", section: "lab", model: "opencode/big-pickle", harness: "opencode", new: true, free: false },
  ]);
});

test("a nudge toward a free model Free folds away opens the picker on it", async () => {
  await open(feed({ nudge: nudge({ model: "opencode/glm-5.3-flash-free" }) }));
  await act(async () => callout("GLM 5.3 Flash").click());
  await settle();

  assert.deepEqual(rows("free"), ["DeepSeek V4.1 Flash", "Inkling 2 Small", "GLM 5.3 Flash"]);
  assert.equal(document.activeElement.textContent, "GLM 5.3 Flash");
});

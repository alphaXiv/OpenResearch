import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { act, createElement, Fragment } from "react";
import { StatusStrip } from "../src/components/StatusStrip.tsx";
import { Toaster } from "../src/components/ui/Toast.tsx";
import { UsageMenu } from "../src/components/UsageMenu.tsx";
import { QueryEvents } from "../src/queries/Events.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, render, serve, settle, streams } from "./render.mjs";

const local = { kind: "local", version: "test", buildChannel: "stable" };
const remoteSession = {
  id: "s", host: "gpu-box", user: "me", status: "connected", version: "test", dashboardProtocol: 1,
  gatewayUrl: "http://localhost:4910/", error: null, repairCommand: null, installPaths: null,
  uiPreferences: { theme: null, locale: null }, canStartNewHost: true,
};
const clover = { loggedIn: true, apiUrl: "https://api.example.org", user: { id: "u", firstName: "Clover", lastName: null, email: "c@example.org", alphaxivUrl: null } };
const signedOut = { loggedIn: false, apiUrl: null, user: null };

const soon = (hours) => new Date(Date.now() + hours * 3_600_000).toISOString();
const window = (remainingPercent, minutes = 10080, model = null) => ({ minutes, model, remainingPercent, resetsAt: soon(30) });
const plan = (harness, name, windows, extra = {}) => ({ harness, plan: name, account: "c@example.org", windows, readAt: new Date().toISOString(), ...extra });
const claudeMax = plan("claude-code", "Claude Ultra", [window(62, 300), window(81), window(40, 10080, "Fable")]);
const chatgptPro = (left) => plan("codex", "ChatGPT Pro", [window(left)]);
const days = ["2026-11-07", "2026-11-08", "2026-11-09", "2026-11-10", "2026-11-11", "2026-11-12", "2026-11-13"];
const report = (fields = {}) => ({ plans: [], days, agents: [], compute: [], ...fields });
const opencodeSpend = { harness: "opencode", currency: "USD", dayMicros: [4_000_000, 14_000_000, 0, 0, 0, 0, 18_000_000], unpriced: 0 };
const modalSpend = { backend: "modal_job", currency: "USD", dayMicros: [0, 0, 4_250_000, 0, 0, 0, 5_500_000], unpriced: 0 };

const harness = (id, name, extra = {}) => ({
  id, name, installed: true, installBroken: false, authenticated: true, authState: "ready", agentReady: true,
  supportsSteering: true, supportsCompaction: true, models: [], ...extra,
});
const catalog = [
  harness("claude-code", "Claude Code"),
  harness("claude-code", "Claude Code", { profile: { id: "lab", name: "Lab", icon: { glyph: "flask-conical", color: "blue" } } }),
  harness("codex", "Codex"),
  harness("opencode", "OpenCode"),
];

let ResizeObservers = [];
afterEach(() => { ResizeObservers = []; });
globalThis.ResizeObserver = class {
  constructor(callback) { this.callback = callback; ResizeObservers.push(this); }
  observe() {}
  disconnect() {}
};

/** Serves `/api/usage` from `usage` (a body, a Response, or a function of the request) and its refresh from `refresh`, recording each request. */
function backend(usage, { account = clover, refresh = { ok: true }, extra = {} } = {}) {
  const requests = [];
  serve((path, init) => {
    requests.push(`${init.method ?? "GET"} ${path}`);
    const route = path.split("?")[0];
    if (route === "/api/usage") return typeof usage === "function" ? usage(path) : usage;
    if (route === "/api/usage/refresh") return typeof refresh === "function" ? refresh() : refresh;
    if (route === "/api/account/openresearch") return account;
    if (route === "/api/alphaxiv/folders") return [];
    if (route === "/api/harnesses") return { harnesses: catalog };
    if (route === "/api/harnesses/setup/commands") return { "claude-code": { install: "install claude", update: "claude update", login: "claude login" } };
    if (extra[route]) return extra[route](path, init);
  });
  return requests;
}

async function strip(usage, { runtime = local, account, refresh, events = false } = {}) {
  const requests = backend(usage, { account, refresh });
  const element = createElement(StatusStrip, { runtime, onDetails() {} });
  const view = await render(events ? createElement(Fragment, null, createElement(QueryEvents), element) : element);
  await settle();
  return { ...view, requests };
}

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};
const usageTrigger = () => [...document.querySelectorAll("button")].find((button) => button.getAttribute("aria-label")?.startsWith(m.usage_on({ machine: "" }).trim()) || button.getAttribute("aria-label")?.includes(m.usage_on({ machine: m.projects_local() })));
const rings = () => [...document.querySelectorAll("[data-plan-ring]")].filter((ring) => !ring.closest(".hidden"));
const panel = () => [...document.querySelectorAll('[role="dialog"]')].at(-1) ?? null;
/** Text as read, without the isolates that keep interpolated values in place. */
const text = (element) => element.textContent.replace(/[\u2066-\u2069]/g, "");
const separators = (container) => container.querySelectorAll('span[aria-hidden="true"].w-px').length;

test("the strip draws a ring per plan, toned by its lowest window, and names each for a screen reader", async () => {
  await strip(report({ plans: [claudeMax, chatgptPro(16)] }));
  assert.deepEqual(rings().map((ring) => ring.dataset.planRing), ["text-text", "text-accent-amber"]);
  const tooltips = [...usageTrigger().parentElement.querySelectorAll('[role="tooltip"]')];
  assert.deepEqual(tooltips.map((tooltip) => tooltip.textContent), [m.usage_title()], "one tooltip for the segment, none per ring");
  const label = usageTrigger().getAttribute("aria-label");
  assert.match(label, /Claude Ultra/);
  assert.match(label, /40% left/);
  assert.match(label, /Fable weekly/);
  assert.match(label, /ChatGPT Pro/);
  assert.match(label, /16% left/);

  await strip(report({ plans: [chatgptPro(4)] }));
  assert.equal(rings().at(-1).dataset.planRing, "text-accent-red");
});

test("a profile's plan wears the profile's own mark, as the model picker draws it", async () => {
  await strip(report({ plans: [claudeMax, { ...chatgptPro(70), harness: "claude-code", profile: "lab", plan: "Claude Pro" }] }));
  const [, lab] = rings();
  assert.ok(lab.querySelector(".text-accent-blue"), "the lab profile's blue glyph tile");
  assert.ok(!rings()[0].querySelector(".text-accent-blue"));
});

test("past three plans the rest fold into +n, amber when a folded plan is low", async () => {
  await strip(report({ plans: [claudeMax, chatgptPro(70), { ...claudeMax, profile: "lab" }, { ...chatgptPro(12), profile: "other" }] }));
  assert.equal(document.querySelectorAll("[data-plan-ring]").length, 3);
  const [wide] = document.querySelectorAll("[data-more-plans]");
  assert.equal(wide.textContent, "+1");
  assert.match(wide.className, /text-accent-amber/);
});

test("a plan that can't be read is a dotted ring, and its row says why with the way forward", async () => {
  const unsupported = plan("claude-code", "Claude Pro", [], { error: { kind: "unsupported", version: "2.0.9" } });
  const failed = { ...chatgptPro(44), error: { kind: "failed", message: "502 from chatgpt.com" } };
  const { requests } = await strip(report({ plans: [unsupported, failed] }));
  assert.deepEqual(rings().map((ring) => ring.dataset.planRing), ["unread", "text-text"]);
  assert.ok(rings()[0].querySelector('circle[stroke-dasharray="2 2.4"]'));

  const before = requests.length;
  await click(usageTrigger());
  assert.deepEqual(requests.slice(before).filter((request) => request.includes("/api/usage")), ["GET /api/usage"], "opening refetches the report and leaves re-reading to the server");
  assert.match(text(panel()), new RegExp(m.usage_plan_unsupported({ agent: "Claude Code", version: "2.0.9" })));
  assert.match(text(panel()), new RegExp(m.usage_plan_failed()));
  assert.match(text(panel()), /44% left/, "the last values stay");

  const reads = requests.length;
  await click(find('[role="dialog"] button', m.usage_try_again()));
  assert.deepEqual(requests.slice(reads), ["POST /api/usage/refresh"]);

  await click(find('[role="dialog"] button', m.usage_update_agent({ agent: "Claude Code" })));
  assert.ok([...document.querySelectorAll('[role="dialog"]')].some((dialog) => dialog.textContent.includes("claude update")), "the agent's own update opens");
});

test("on a remote machine there is no update button, since setup runs where the agent does", async () => {
  const unsupported = plan("claude-code", "Claude Pro", [], { error: { kind: "unsupported", version: "2.0.9" } });
  await strip(report({ plans: [unsupported] }), { runtime: { ...local, kind: "ssh", dashboardProtocol: 1, session: remoteSession } });
  await click(usageTrigger());
  assert.match(text(panel()), /2\.0\.9/);
  assert.equal(find('[role="dialog"] button', m.usage_update_agent({ agent: "Claude Code" })), null);
});

test("API keys only: the gauge and Usage, a graph of spend, and no plans section", async () => {
  await strip(report({ agents: [opencodeSpend], compute: [modalSpend] }));
  assert.equal(rings().length, 0);
  assert.match(usageTrigger().textContent, new RegExp(m.usage_title()));

  await click(usageTrigger());
  assert.equal(panel().querySelector("[data-plan-ring]"), null);
  assert.match(text(panel()), /\$36\.00/, "OpenCode's week");
  assert.match(text(panel()), new RegExp(m.usage_at_api_prices()));
  assert.match(text(panel()), new RegExp(m.usage_compute_week()));
  assert.match(text(panel()), /\$9\.75/);

  assert.ok(panel().querySelector('[data-spend-graph="ai"]'));
  await click(find('[role="dialog"] button', m.usage_graph_compute()));
  assert.ok(panel().querySelector('[data-spend-graph="compute"]'));
  assert.equal(find('[role="dialog"] button[aria-pressed="true"]').textContent, m.usage_graph_compute());
});

test("the graph's switch appears only when both AI and compute have data; estimates and unpriced runs say so", async () => {
  await strip(report({ compute: [modalSpend, { backend: "hf_job", currency: "USD", dayMicros: [0, 0, 0, 0, 0, 0, 0], unpriced: 2 }] }));
  await click(usageTrigger());
  assert.equal(panel().querySelector('[role="group"]'), null);
  assert.ok(panel().querySelector('[data-spend-graph="compute"]'));
  assert.match(text(panel()), /~\$9\.75/);
  assert.match(text(panel()), new RegExp(m.usage_cost_unavailable()));
  assert.doesNotMatch(text(panel()), /\$0\.00/);
});

test("a backend with priced and unpriced runs says so beside its partial amount", async () => {
  await strip(report({ compute: [{ ...modalSpend, unpriced: 1 }] }));
  await click(usageTrigger());
  const partial = `~$9.75 · ${m.usage_runs_unpriced_one({ count: 1 })}`;
  assert.equal(text(panel()).split(partial).length - 1, 2, "in the compute row and the graph's legend");
});

test("usage that can't be read keeps the segment, dimmed, and its panel says why with Try again", async () => {
  let failing = true;
  await strip(() => failing ? Response.json({ error: "Couldn't open the usage store." }, { status: 500 }) : report({ plans: [claudeMax] }));
  assert.match(usageTrigger().className, /opacity-50/);
  await click(usageTrigger());
  assert.match(text(panel()), /Couldn't open the usage store\./);

  failing = false;
  await click(find('[role="dialog"] button', m.usage_try_again()));
  assert.equal(rings().length, 1);
  assert.doesNotMatch(usageTrigger().className, /opacity-50/);

  // A read that fails after one that worked keeps the last values, dimmed, with the error and Try again.
  failing = true;
  await click(usageTrigger());
  await click(usageTrigger());
  assert.match(usageTrigger().className, /opacity-50/);
  assert.equal(rings().length, 1, "the last plan stays");
  assert.match(text(panel()), /Couldn't open the usage store\./);
  assert.ok(find('[role="dialog"] button', m.usage_try_again()));
  assert.doesNotMatch(text(panel()), new RegExp(m.usage_updated_now()));
});

test("an idle strip reads usage again every five minutes", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const { requests } = await strip(report({ plans: [claudeMax] }));
  const reads = () => requests.filter((request) => request === "GET /api/usage").length;
  const before = reads();
  await act(async () => t.mock.timers.tick(4 * 60_000));
  await settle();
  assert.equal(reads(), before);
  await act(async () => t.mock.timers.tick(60_000));
  await settle();
  assert.equal(reads(), before + 1);
});

test("nothing to show leaves no usage segment, and an older orx up without the route the same", async () => {
  const { container } = await strip(report());
  assert.equal(usageTrigger(), undefined);
  assert.equal(separators(container), 1);

  await strip(Response.json({ error: "route not found" }, { status: 404 }));
  assert.equal(usageTrigger(), undefined);
});

test("plans not read yet are unknown: the segment stays and the panel says it's reading", async () => {
  await strip(report({ plans: null }));
  await click(usageTrigger());
  assert.match(text(panel()), new RegExp(m.usage_reading_plans()));
});

test("signed out, the strip offers sign-in in the account's place", async () => {
  const { container } = await strip(report({ plans: [claudeMax] }), { account: signedOut });
  assert.equal(container.querySelector("button").textContent, m.settings_sign_in());
});

test("a remote machine's blue fill takes no hairline beside it", async () => {
  const { container } = await strip(report({ plans: [claudeMax] }), { runtime: { ...local, kind: "ssh", dashboardProtocol: 1, session: remoteSession } });
  const machine = container.querySelector("[data-machine-switcher]");
  assert.match(machine.className, /bg-accent-blue-subtle/);
  assert.equal(separators(container), 1);
  assert.match(usageTrigger().getAttribute("aria-label"), /gpu-box/);

  const { container: home } = await strip(report({ plans: [claudeMax] }));
  assert.equal(separators(home), 2);
});

test("offline, the machine turns amber, the agents dim and the panel says when it last knew", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { container } = await strip(report({ plans: [claudeMax] }), { events: true });
  await act(async () => {
    streams[0].onerror();
    t.mock.timers.tick(8_000);
  });
  t.mock.timers.reset();
  await settle();

  const machine = container.querySelector("[data-machine-switcher]");
  assert.match(machine.className, /text-accent-amber/);
  assert.equal(machine.getAttribute("aria-label"), m.remote_machine_switcher_status({ machine: m.projects_local(), status: m.remote_machine_disconnected() }));
  assert.match(usageTrigger().className, /opacity-50/);
  await click(usageTrigger());
  assert.match(text(panel()), new RegExp(m.usage_as_of({ time: "" }).trim()));
});

test("a finished re-read announced on the stream refetches the report", async () => {
  let left = 80;
  await strip(() => report({ plans: [chatgptPro(left)] }), { events: true });
  assert.match(usageTrigger().getAttribute("aria-label"), /80% left/);
  left = 15;
  await act(async () => streams[0].dispatchEvent(new Event("usage.changed")));
  await settle();
  assert.match(usageTrigger().getAttribute("aria-label"), /15% left/);
});

/** Lays the strip out: the machine's name container `machineWidth` wide around a label `labelWidth` wide, and the account name `nameWidth`. */
function layout(container, { machineWidth, labelWidth, nameWidth, nameRoom = nameWidth }) {
  const machine = container.querySelector("[data-machine-name]");
  const rect = (left, width) => () => ({ left, right: left + width, width, top: 0, bottom: 28, height: 28 });
  machine.getBoundingClientRect = rect(0, machineWidth);
  const label = machine.firstElementChild;
  label.getBoundingClientRect = rect(0, Math.min(labelWidth, machineWidth));
  Object.defineProperty(label, "scrollWidth", { configurable: true, value: labelWidth });
  Object.defineProperty(label, "clientWidth", { configurable: true, value: Math.min(labelWidth, machineWidth) });
  const name = container.querySelector("[data-account-name]");
  if (name) {
    Object.defineProperty(name, "scrollWidth", { configurable: true, value: nameWidth });
    Object.defineProperty(name, "clientWidth", { configurable: true, value: nameRoom });
  }
  const ruler = container.querySelector("[data-account-ruler]");
  if (ruler) Object.defineProperty(ruler, "offsetWidth", { configurable: true, value: nameWidth });
}

const resize = () => act(async () => ResizeObservers.at(-1).callback([]));
const shownName = (container) => container.querySelector("[data-account-name]")?.textContent ?? null;

test("your name shows only whole, and gives way before the machine's name does", async () => {
  const { container } = await strip(report({ plans: [claudeMax] }));
  assert.equal(shownName(container), "Clover");

  layout(container, { machineWidth: 120, labelWidth: 160, nameWidth: 44 });
  await resize();
  assert.equal(shownName(container), null, "a clipped machine name takes the room back");
  assert.match(container.querySelector("[data-account-ruler]").textContent, /Clover/);

  layout(container, { machineWidth: 210, labelWidth: 160, nameWidth: 44 });
  await resize();
  assert.equal(shownName(container), null, "50px of slack doesn't hold the name and its margin");

  layout(container, { machineWidth: 230, labelWidth: 160, nameWidth: 44 });
  await resize();
  assert.equal(shownName(container), "Clover");

  layout(container, { machineWidth: 160, labelWidth: 160, nameWidth: 44, nameRoom: 30 });
  await resize();
  assert.equal(shownName(container), null, "a name that would show cut off doesn't show");
});

async function composer(props, usage = report({ plans: [claudeMax] })) {
  const requests = backend(usage);
  const compacted = [];
  const menu = (next) => createElement(UsageMenu, {
    harness: "claude-code", busy: false, profile: null, sessionId: "chat-1", remote: false, onCompact: () => compacted.push(true), ...next,
  });
  const view = await render(menu(props));
  await settle();
  const trigger = () => view.container.querySelector("[data-usage-trigger]");
  const rerender = async (next) => {
    await view.rerender(menu(next));
    await settle();
  };
  return { ...view, requests, compacted, trigger, rerender };
}

test("the composer's ring is context; its menu shows context, the reserve before compacting and Compact", async () => {
  const { trigger, compacted, requests } = await composer({ usage: { usedTokens: 112_000, contextWindow: 200_000, compactAt: 167_000 } }, report({ plans: [claudeMax], chat: { agent: [{ micros: 6_840_000, currency: "USD" }], compute: null } }));
  assert.ok(trigger().querySelector("svg circle"));
  assert.equal(trigger().textContent, "", "a plan with room and a subscription's spend stay in the menu");

  await click(trigger());
  assert.deepEqual(requests.filter((request) => request.includes("/api/usage")), ["GET /api/usage?chat=chat-1", "GET /api/usage?chat=chat-1"], "opening refetches, never forcing a re-read");
  const menu = panel();
  assert.match(text(menu), /112k of 200k/);
  assert.match(text(menu), /55k left before compacting/);
  assert.ok(menu.querySelector("[data-compact-at]"));
  assert.match(text(menu), /Claude Ultra/);
  assert.doesNotMatch(text(menu), /\$6\.84/, "a subscription's would-be API price stays out");

  await click(find('[role="dialog"] button', m.usage_compact()));
  assert.equal(compacted.length, 1);
  assert.equal(panel(), null);
});

test("a plan window at 20% or under rides on the trigger", async () => {
  const { trigger } = await composer({ harness: "codex", usage: { usedTokens: 61_000, contextWindow: 258_000 } }, report({ plans: [claudeMax, chatgptPro(16)] }));
  assert.equal(trigger().textContent, "16% left");
  assert.match(trigger().querySelector("span").className, /text-accent-amber/);
});

const spent = (micros, plans = [claudeMax]) => report({ plans, chat: { agent: [{ micros, currency: "USD" }], compute: null } });

test("a pay-per-token chat shows its spend; an agent without compaction has no Compact", async () => {
  const { trigger } = await composer({ harness: "opencode", onCompact: undefined, usage: { usedTokens: 34_000, contextWindow: 128_000 } }, spent(830_000));
  assert.equal(trigger().textContent, "$0.83");
  await click(trigger());
  assert.match(text(panel()), new RegExp(`${m.usage_chat()}\\$0\\.83`));
  assert.doesNotMatch(text(panel()), /Claude Max/, "the plan of another account stays out");
  assert.equal(find('[role="dialog"] button', m.usage_compact()), null);
});

test("while the plans are unread, whether a chat pays per token is unknown, so its spend waits", async () => {
  const { trigger } = await composer({ harness: "opencode", usage: { usedTokens: 34_000, contextWindow: 128_000 } }, spent(830_000, null));
  assert.equal(trigger().textContent, "");
  await click(trigger());
  assert.doesNotMatch(text(panel()), /\$0\.83/);
});

test("the chat's spend is read again as each turn ends, not while it streams", async () => {
  let micros = 830_000;
  const props = { harness: "opencode", busy: true, usage: { usedTokens: 34_000, contextWindow: 128_000 } };
  const { trigger, requests, rerender } = await composer(props, () => spent(micros));
  const reads = () => requests.filter((request) => request === "GET /api/usage?chat=chat-1").length;
  micros = 1_240_000;
  await rerender({ ...props, usage: { ...props.usage, usedTokens: 36_000 } });
  assert.equal(reads(), 1, "a streamed token count reads nothing");
  await rerender({ ...props, busy: false });
  assert.equal(reads(), 2);
  assert.equal(trigger().textContent, "$1.24");
});

test("before the first message the gauge stands in, only while the menu has a plan to show", async () => {
  const { trigger } = await composer({ sessionId: null, usage: undefined });
  assert.equal(trigger().querySelector("circle"), null);
  assert.ok(trigger().querySelector("path"));
  await click(trigger());
  assert.match(text(panel()), /Claude Ultra/);

  const empty = await composer({ harness: "opencode", sessionId: null, usage: undefined });
  assert.equal(empty.trigger(), null);
});

test("this chat's compute: a live run so far, and an unpriced one is never $0", async () => {
  const running = { runs: 3, costMicros: 1_200_000, currency: "USD", running: true, unpriced: 0 };
  const { trigger } = await composer({ usage: { usedTokens: 10_000, contextWindow: 200_000 } }, report({ plans: [], chat: { agent: [], compute: running } }));
  await click(trigger());
  assert.match(text(panel()), /Compute · 3 runs/);
  assert.match(text(panel()), /~\$1\.20 so far/);
  assert.ok(panel().querySelector("[data-running]"));

  const unpriced = await composer({ usage: { usedTokens: 10_000, contextWindow: 200_000 } }, report({ plans: [], chat: { agent: [], compute: { ...running, runs: 1, costMicros: 0, running: false, unpriced: 1 } } }));
  await click(unpriced.trigger());
  assert.match(text(panel()), /Compute · 1 run/);
  assert.match(text(panel()), new RegExp(m.usage_cost_unavailable()));
});

test("against an older orx up the composer keeps the plain context ring", async () => {
  const { trigger } = await composer({ usage: { usedTokens: 50_000, contextWindow: 200_000 } }, Response.json({ error: "route not found" }, { status: 404 }));
  assert.ok(trigger().querySelector("circle"));
  await click(trigger());
  assert.match(text(panel()), /50k of 200k/);
  assert.doesNotMatch(text(panel()), /Claude/);
});

test("Refresh and Try again force a re-read, and a failure shows where it was clicked", async () => {
  const failed = { ...chatgptPro(44), error: { kind: "failed", message: "502 from chatgpt.com" } };
  const refusal = "Couldn't start reading your plans.";
  const { requests } = await strip(report({ plans: [failed] }), { refresh: () => Response.json({ error: refusal }, { status: 500 }) });
  await click(usageTrigger());

  const before = requests.length;
  await click(find('[role="dialog"] button', m.usage_try_again()));
  assert.deepEqual(requests.slice(before), ["POST /api/usage/refresh"]);
  const alerts = () => [...panel().querySelectorAll('[role="alert"]')];
  assert.equal(alerts().length, 1);
  assert.equal(alerts()[0].textContent, refusal);
  assert.ok(alerts()[0].parentElement.contains(find('[role="dialog"] button', m.usage_try_again())), "under the plan's row");

  const refresh = panel().querySelector(`button[aria-label="${m.usage_refresh()}"]`);
  await click(refresh);
  assert.deepEqual(requests.slice(before), ["POST /api/usage/refresh", "POST /api/usage/refresh"]);
  assert.equal(alerts().length, 2);
  const heading = alerts().find((alert) => alert.previousElementSibling?.contains(refresh));
  assert.equal(heading?.textContent, refusal, "under the panel's heading");
});

test("the composer refetches a running run every 10 seconds, only while its menu is open", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const running = { runs: 1, costMicros: 400_000, currency: "USD", running: true, unpriced: 0 };
  const { trigger, requests } = await composer({ usage: { usedTokens: 10_000, contextWindow: 200_000 } }, report({ plans: [], chat: { agent: [], compute: running } }));
  const reads = () => requests.filter((request) => request === "GET /api/usage?chat=chat-1").length;
  const passes = async (ms) => {
    await act(async () => t.mock.timers.tick(ms));
    await settle();
  };

  await passes(10_000);
  assert.equal(reads(), 1, "closed, nothing polls");

  await click(trigger());
  assert.equal(reads(), 2);
  await passes(10_000);
  assert.equal(reads(), 3);

  await click(trigger());
  assert.equal(panel(), null);
  await passes(30_000);
  assert.equal(reads(), 3);

  const done = await composer({ usage: { usedTokens: 10_000, contextWindow: 200_000 } }, report({ plans: [], chat: { agent: [], compute: { ...running, running: false } } }));
  const doneReads = () => done.requests.filter((request) => request === "GET /api/usage?chat=chat-1").length;
  await click(done.trigger());
  await passes(30_000);
  assert.equal(doneReads(), 2, "a finished run isn't polled");

  t.mock.timers.reset();
});

test("this chat's compute reads cost unavailable only when no run has a price", async () => {
  const compute = (fields) => report({ plans: [], chat: { agent: [], compute: { runs: 1, costMicros: 0, currency: "USD", running: false, unpriced: 0, ...fields } } });
  const line = async (fields) => {
    const { trigger } = await composer({ usage: { usedTokens: 10_000, contextWindow: 200_000 } }, compute(fields));
    await click(trigger());
    return text(panel());
  };

  const waiting = await line({});
  assert.match(waiting, /Compute · 1 run/);
  assert.doesNotMatch(waiting, new RegExp(m.usage_cost_unavailable()), "a priced run that hasn't started yet");
  assert.doesNotMatch(waiting, /\$0\.00/);

  assert.match(await line({ runs: 3, costMicros: 1_200_000, unpriced: 1 }), /Compute · 3 runs~\$1\.20 · 1 run unpriced/);
  assert.match(await line({ runs: 3, costMicros: 1_200_000, running: true, unpriced: 2 }), /~\$1\.20 so far · 2 runs unpriced/);
  assert.match(await line({ runs: 2, unpriced: 2 }), new RegExp(`Compute · 2 runs${m.usage_cost_unavailable()}`));
});

test("the graph's days are readable without seeing the bars", async () => {
  await strip(report({ agents: [opencodeSpend] }));
  await click(usageTrigger());
  const weekday = (date) => new Intl.DateTimeFormat("en", { weekday: "short" }).format(new Date(2026, 10, date));
  assert.deepEqual([...panel().querySelectorAll("li")].map(text), [
    m.usage_day_amount({ day: weekday(7), amount: "$4.00" }),
    m.usage_day_amount({ day: weekday(8), amount: "$14.00" }),
    m.usage_day_amount({ day: m.usage_today(), amount: "$18.00" }),
  ], "the days with spend, as the bars show them");
});

test("an agent too old to report usage, version unknown, still offers its update", async () => {
  const unsupported = plan("claude-code", "Claude Pro", [], { error: { kind: "unsupported", version: "" } });
  await strip(report({ plans: [unsupported] }));
  await click(usageTrigger());
  assert.match(text(panel()), /Claude Code can't report usage\./);
  await click(find('[role="dialog"] button', "Update Claude Code"));
  assert.ok([...document.querySelectorAll('[role="dialog"]')].some((dialog) => dialog.textContent.includes("claude update")));
});

const fullReset = { id: "credit-1", title: "Full reset", description: "Thanks for using Codex! You've been granted one free rate limit reset.", expiresAt: "2026-12-12T22:36:31.000Z" };
const withReset = (left, resets = { available: 1, credits: [fullReset] }) => ({ ...chatgptPro(left), resets });
const resetLine = () => document.querySelector("[data-plan-resets]");
const useButton = () => resetLine()?.querySelector("button") ?? null;
const confirmDialog = () => [...document.querySelectorAll('[role="dialog"]')].find((dialog) => dialog.textContent.includes(m.usage_reset_effect())) ?? null;
const confirmButton = () => [...(confirmDialog()?.querySelectorAll("button") ?? [])].find((button) => button.textContent === m.usage_reset_confirm());
const confirmReset = () => click(confirmButton());

/** The strip, its panel open, with `answer(body)` replying to each reset; the bodies sent land in `resets`. */
async function resetPanel(answer, plans = [withReset(95)]) {
  const resets = [];
  backend(report({ plans }), {
    extra: {
      "/api/usage/reset": (_path, init) => {
        const body = JSON.parse(init.body);
        resets.push(body);
        return answer(body);
      },
    },
  });
  const view = await render(createElement(Fragment, null, createElement(Toaster), createElement(StatusStrip, { runtime: local, onDetails() {} })));
  await settle();
  await click(usageTrigger());
  return { resets, unmount: view.unmount };
}

/** Opens the reset's dialog from wherever the plan's line is: a press in the dialog closes the panel behind it. */
async function openReset() {
  if (!resetLine()) await click(usageTrigger());
  await click(useButton());
}

test("a plan's line names its reset and when to use it by, only while one is available", async () => {
  const first = await strip(report({ plans: [withReset(95), claudeMax] }));
  await click(usageTrigger());
  const useBy = new Intl.DateTimeFormat("en", { month: "short", day: "numeric" }).format(Date.parse(fullReset.expiresAt));
  assert.match(text(resetLine()), new RegExp(m.usage_reset_use_by({ reset: "Full reset", date: useBy })));
  assert.equal(document.querySelectorAll("[data-plan-resets]").length, 1, "Claude's plan has none");
  assert.equal(useButton().getAttribute("aria-label"), m.usage_reset_use_named({ reset: "Full reset" }));
  await first.unmount();

  const counted = await strip(report({ plans: [withReset(95, { available: 2, credits: null })] }));
  await click(usageTrigger());
  assert.equal(text(resetLine()).replace(useButton().textContent, ""), "2 resets", "only the count is known");
  await counted.unmount();

  await strip(report({ plans: [chatgptPro(95)] }));
  await click(usageTrigger());
  assert.equal(resetLine(), null);
});

test("the composer's menu offers the chat's plan's reset too", async () => {
  const { trigger } = await composer({ harness: "codex", usage: { usedTokens: 10_000, contextWindow: 258_000 } }, report({ plans: [withReset(40)] }));
  await click(trigger());
  assert.match(text(resetLine()), /Full reset/);
  await click(useButton());
  assert.match(text(confirmDialog()), /Weekly: 40% left/);
});

test("Use reset confirms with the reset's own words and the plan's lowest window; Cancel spends nothing", async () => {
  const { resets } = await resetPanel(() => ({ outcome: "reset" }), [{ ...withReset(95), windows: [window(95), window(60, 300)] }]);
  await openReset();
  const dialog = confirmDialog();
  assert.match(text(dialog), new RegExp(m.usage_reset_title_named({ reset: "Full reset" }).replace("?", "\\?")));
  assert.match(text(dialog), /granted one free rate limit reset/);
  assert.match(text(dialog), new RegExp(m.usage_reset_effect()));
  assert.match(text(dialog), /5-hour: 60% left/, "the window closest to running out");

  await click(find('[role="dialog"] button', m.common_cancel()));
  await settle();
  assert.deepEqual(resets, []);
});

test("a reset closes the dialog and says so", async () => {
  for (const outcome of ["reset", "alreadyRedeemed"]) {
    const { resets, unmount } = await resetPanel(() => ({ outcome }));
    await openReset();
    await confirmReset();
    assert.deepEqual(resets, [{ creditId: "credit-1", attempt: resets[0].attempt }], outcome);
    assert.equal(confirmDialog(), null, `${outcome} closes the dialog`);
    assert.ok(find("[data-sonner-toast]", m.usage_reset_done()), outcome);
    await unmount();
  }
});

test("a reset that couldn't happen is said in the dialog, which stays open", async () => {
  for (const [answer, message] of [
    [() => ({ outcome: "nothingToReset" }), m.usage_reset_nothing()],
    [() => ({ outcome: "noCredit" }), m.usage_reset_none_left()],
    [() => Response.json({ error: "Codex 0.150.0 can't use resets. Update Codex, then try again." }, { status: 409 }), "Codex 0.150.0 can't use resets. Update Codex, then try again."],
  ]) {
    const { unmount } = await resetPanel(answer);
    const toasts = () => document.querySelectorAll("[data-sonner-toast]").length;
    await openReset();
    const before = toasts();
    await confirmReset();
    assert.equal(confirmDialog()?.querySelector('[role="alert"]')?.textContent, message);
    assert.ok(toasts() <= before, "no success over a reset that didn't happen");
    await unmount();
  }
});

test("a retried attempt sends the same key, so it can't spend two resets; a new attempt gets a new one", async () => {
  let answer = () => Response.json({ error: "Codex didn't answer within 15 seconds." }, { status: 502 });
  const { resets } = await resetPanel(() => answer());
  await openReset();
  await confirmReset();
  assert.ok(confirmDialog(), "the failure keeps the dialog open to retry");
  answer = () => ({ outcome: "nothingToReset" });
  await confirmReset();
  assert.equal(resets.length, 2);
  assert.ok(resets[0].attempt);
  assert.equal(resets[1].attempt, resets[0].attempt);

  answer = () => ({ outcome: "reset" });
  await confirmReset();
  assert.notEqual(resets[2].attempt, resets[0].attempt, "an answered attempt is over");
  await openReset();
  await confirmReset();
  assert.equal(resets.length, 4);
  assert.notEqual(resets[3].attempt, resets[2].attempt);
});

test("while a reset is pending, its button can't send another", async () => {
  let answer;
  const { resets } = await resetPanel(() => new Promise((resolve) => { answer = resolve; }));
  await openReset();
  await act(async () => confirmButton().click());
  const pending = confirmButton();
  assert.equal(pending.disabled, true);
  await act(async () => pending.click());
  assert.equal(resets.length, 1);

  await act(async () => answer({ outcome: "reset" }));
  await settle();
  assert.equal(confirmDialog(), null);
});

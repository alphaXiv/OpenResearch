import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test, { afterEach } from "node:test";
import { act, createElement, Fragment } from "react";
import { timeAgo } from "../src/api.ts";
import { ChatImageScope } from "../src/components/Md.tsx";
import { RunWatchingRow } from "../src/components/SettingsPage.tsx";
import { StatusReport } from "../src/components/StatusReport.tsx";
import { Toaster } from "../src/components/ui/Toast.tsx";
import { fmtClock } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { QueryEvents } from "../src/queries/Events.tsx";
import { createWorkspace } from "../src/workspace.tsx";
import { codex, composer, refused, renderChat, session } from "./chat.mjs";
import { model } from "./models.mjs";
import { find, press, render, serve, settle, streams } from "./render.mjs";

const running = JSON.parse(readFileSync(new URL("./fixtures/runMonitor/running.json", import.meta.url), "utf8"));
const now = Date.now();
const body = [
  "Comparing the best two learning rates.",
  "",
  "## Done",
  "- Baseline reached 2.31 loss.",
  "- 3e-4 beat 1e-3 by 0.08 loss.",
  "",
  "## Next",
  "Rerun 3e-4 with three seeds.",
].join("\n");
const report = (extra = {}) => ({ body, version: 1, read: false, updatedAt: now - 120_000, enabled: true, needsModel: null, writing: false, error: null, ...extra });

/** Serves chat s's status as `state.report` holds it; `state.refuse` answers a write with an error. Every request is kept. */
function backend(state, routes = {}) {
  const requests = [];
  serve((path, init) => {
    const request = `${init.method ?? "GET"} ${path}`;
    const body = init.body ? JSON.parse(init.body) : undefined;
    requests.push({ request, body });
    if (routes[request]) return routes[request]();
    if (request === "GET /api/settings/ui-state") return { onboardingCompleted: true, ...state.uiState };
    if (request === "POST /api/settings/ui-state") {
      state.uiState = { ...state.uiState, ...body };
      return { onboardingCompleted: true, ...state.uiState };
    }
    if (request === "GET /api/chat/sessions/s/report") return state.report;
    const refusal = state.refuse?.[request];
    if (refusal) return Response.json({ error: refusal.error }, { status: refusal.status });
    if (request === "POST /api/chat/sessions/s/report/enabled") {
      state.report = { ...state.report, enabled: body.enabled };
      return state.report;
    }
    if (request === "POST /api/chat/sessions/s/report/read") {
      state.report = { ...state.report, read: state.report.version === body.version };
      return state.report;
    }
    if (request === "POST /api/chat/sessions/s/report/refresh") {
      state.report = { ...state.report, writing: false, error: null };
      return state.report;
    }
  });
  return requests;
}

afterEach(() => {
  sessionStorage.clear();
  localStorage.clear();
});

/** Chat s's card, opened earlier in this tab when `state.open`. */
async function card(state, routes) {
  const requests = backend(state, routes);
  const workspace = createWorkspace("test");
  if (state.open) sessionStorage.setItem(`status-open:${workspace.url("/api/")}:s`, "1");
  const report = createElement(ChatImageScope, { projectId: "p", sessionId: "s" }, createElement(StatusReport, { sessionId: "s", onTurnOff() {}, onOpenSettings() {}, onOpenRunPage: (run, intent) => state.opened?.push([run.id, intent]) }));
  const view = await render(createElement(Fragment, null, createElement(Toaster), createElement(QueryEvents), report), workspace);
  await settle();
  return { ...view, requests };
}

const plain = (text) => text.replace(/[\u2066-\u2069]/g, "");
const emit = async (data) => {
  await act(async () => streams[0].dispatchEvent(new MessageEvent("chat.report", { data: JSON.stringify(data) })));
  await settle();
};
const section = () => document.querySelector(`section[aria-label="${m.status_report_label()}"]`);
const button = (label) => find(`button[aria-label="${label}"]`);
const click = async (element) => { await act(async () => element.click()); await settle(); };
const opened = () => section().querySelector(".overflow-y-auto .md");

test("folded, the card shows the first line, the time and its age; the chevron and Escape open and fold it", async () => {
  const state = { report: report() };
  const { requests, unmount } = await card(state);
  assert.ok(section().textContent.includes("Comparing the best two learning rates."));
  assert.ok(plain(section().textContent).includes(`${fmtClock(now - 120_000)} · ${timeAgo(now - 120_000)}`));
  assert.equal(opened(), null, "the body stays folded");
  assert.ok(!section().textContent.includes(m.status_report_offer()), "a status with a model offers none");
  assert.ok(!section().textContent.includes("Rerun 3e-4"));

  await click(button(m.common_show_more()));
  assert.equal(requests.filter((r) => r.request.startsWith("POST")).length, 0, "opening saves nothing on the server");
  const doc = opened();
  assert.equal(doc.querySelector("p").textContent, "Comparing the best two learning rates.", "the first line opens the document");
  assert.ok(doc.textContent.includes("Rerun 3e-4 with three seeds."));
  assert.equal(section().textContent.split("Comparing the best two learning rates.").length, 2, "the first line shows once");

  doc.closest("section").querySelector("button").focus();
  await press("Escape");
  await settle();
  assert.equal(opened(), null);
  assert.equal(document.activeElement, button(m.common_show_more()), "focus lands on the chevron");
  await unmount();
});

test("Mark read folds the card until a newer version arrives", async () => {
  const state = { open: true, report: report() };
  const { requests } = await card(state);
  assert.ok(opened());

  await click(find("button", m.status_report_mark_read()));
  assert.deepEqual(requests.filter((r) => r.request.endsWith("/report/read")).map((r) => r.body), [{ version: 1 }]);
  assert.equal(opened(), null, "reading folds it");
  assert.equal(document.activeElement, button(m.common_show_more()), "focus lands on the chevron");
  assert.equal(find("button", m.status_report_mark_read()), null);
  assert.ok(section().textContent.includes("Comparing the best two learning rates."), "the folded line still says what it read");

  await emit({ sessionId: "s", report: report({ body: "Seeds agree on 3e-4.", version: 2, updatedAt: Date.now() }) });
  assert.ok(find("button", m.status_report_mark_read()), "a newer version is unread");
});

test("dragging the open card's top edge resizes it, for every chat", async () => {
  const state = { open: true, report: report() };
  const first = await card(state);
  const grip = find(`[role="separator"][aria-label="${m.status_report_resize()}"]`);
  await act(async () => grip.focus());
  await press("ArrowDown");
  await settle();
  const height = opened().closest(".overflow-y-auto").style.height;
  assert.match(height, /^\d+px$/);
  await first.unmount();

  await card(state);
  assert.equal(opened().closest(".overflow-y-auto").style.height, height, "the height is kept");
});

test("a card opened in one chat stays open in that chat for the tab, and folded in another", async () => {
  const state = { report: report() };
  const first = await card(state);
  await click(button(m.common_show_more()));
  await first.unmount();

  await card(state);
  assert.ok(opened(), "reopening the chat keeps it open");

  const other = createElement(StatusReport, { sessionId: "t", onTurnOff() {} });
  serve((path) => path === "/api/chat/sessions/t/report" ? report() : path === "/api/settings/ui-state" ? { onboardingCompleted: true } : undefined);
  await render(other);
  await settle();
  const cards = document.querySelectorAll(`section[aria-label="${m.status_report_label()}"]`);
  assert.equal(cards.length, 2);
  assert.equal(cards[1].querySelector(".overflow-y-auto .md"), null, "another chat starts folded");
});

test("clicking the folded line opens the card", async () => {
  const state = { report: report() };
  await card(state);
  await click(find("section p", "Comparing the best"));
  assert.ok(opened());
});

const cosineRun = (id, experimentId, extra = {}) => ({
  id, experimentId, projectId: "p", chatSessionId: "s", status: "running", createdAt: now - 60_000, startedAt: now - 60_000, updatedAt: now, contact: null, cancelling: false, lostReason: null, host: null, backend: { kind: "local_job" },
  monitor: { tone: "running", rev: running.rev, generation: running.generation, asOf: running.asOf, sentence: running.sentence, flag: null }, ...extra,
});
const experiment = (id, title) => ({ id, projectId: "p", slug: id, branchName: id, title, runCommand: "", agentStatus: "", createdAt: 1, updatedAt: 1 });
/** Runs r1 (Cosine decay) and r0 (Baseline) in project p; r1's monitor is `monitor`. */
const figureRoutes = (monitor) => ({
  "GET /api/projects/p/runs": () => ({ runs: [cosineRun("r1", "e1"), cosineRun("r0", "e0", { status: "succeeded" })] }),
  "GET /api/projects/p/experiments": () => ({ experiments: [experiment("e1", "Cosine decay"), experiment("e0", "Baseline")] }),
  "GET /api/runs/r1/monitor": () => monitor,
});

test("open, the status renders its Markdown and figures, each result linked to its experiment in place of a chip", async () => {
  const cited = [
    "Cosine decay **beats** the baseline.",
    "",
    "***The val split leaks into train.***",
    "",
    "| Arm | Val loss |",
    "| --- | --- |",
    "| cosine | 0.0119 |",
    "",
    'Cosine decay **lowers val loss to <run id="r1" label="0.0119"/>** from <run id="r0" label="0.0124"/>, changing only <file path="src/train.py" lines="20-40"/>.',
    "",
    '**No gain** from <run id="r0"/>.',
    "",
    '<run id="r1" figure/>',
    "",
    "![loss curve](artifacts/loss.png)",
  ].join("\n");
  const state = { open: true, report: report({ body: cited }), opened: [] };
  await card(state, figureRoutes(running));
  await settle();
  const doc = opened();
  assert.equal(doc.querySelector("strong").textContent, "beats");
  assert.equal(doc.querySelector("em > strong, strong > em").textContent, "The val split leaks into train.", "a warning in bold italic");
  assert.equal(doc.querySelector("td").textContent, "cosine");
  assert.equal(doc.querySelector(".run-chip"), null, "no chips");
  const links = () => [...doc.querySelectorAll("button.file-link")].map((link) => plain(link.textContent));
  assert.deepEqual(links(), ["0.0119", "0.0124", "Baseline"], "each result links in place, and an unlabeled run by its experiment's name");
  assert.equal(plain(find(".md button.file-link", "0.0119").getAttribute("aria-description")), m.a11y_open_experiment({ name: "Cosine decay" }), "a result says which experiment it opens");
  assert.ok(doc.querySelector("strong button.file-link"), "a result keeps the bold around it");
  await click(find(".md button.file-link", "0.0119"));
  await click(find(".md button.file-link", "Baseline"));
  assert.deepEqual(state.opened, [["r1", "preview"], ["r0", "preview"]], "a link opens its run's experiment");
  assert.equal(doc.querySelector(".file-chip:not(.run-chip)").textContent, "train.py:20");
  assert.ok(doc.querySelector('svg[role="img"][aria-label*="Loss"]'), "the cited run's chart");
  const image = doc.querySelector('img[alt="loss curve"]');
  assert.ok(image.getAttribute("src").includes(encodeURIComponent("loss.png")));
  assert.ok(!doc.textContent.includes("<run") && !doc.textContent.includes("<file") && !doc.textContent.includes("**"), doc.textContent);
});

test("a run tag the project can't resolve yet reads as plain text, never a dead link", async () => {
  const state = { open: true, report: report({ body: 'Cosine decay reached <run id="r" label="0.0119"/>, and <run id="r9" label="0.0130"/> is gone.' }) };
  await card(state, figureRoutes(running));
  const doc = opened();
  assert.ok(doc.textContent.includes("0.0119") && doc.textContent.includes("0.0130"));
  assert.equal(doc.querySelector("button.file-link"), null, "an ambiguous prefix and an unknown run link nowhere");
});

test("the folded line shows a run tag as text, as the open status does", async () => {
  await card({ report: report({ body: 'Cosine decay reached <run id="r1" label="0.0119"/>.\n\nMore.' }) }, figureRoutes(running));
  assert.ok(section().textContent.includes("0.0119"));
  assert.equal(section().querySelector(".run-chip"), null);
});

test("a figure of a run with no chart to draw leaves no empty frame", async () => {
  const statsOnly = { ...running, view: { ...running.view, blocks: [{ stats: ["loss"] }] } };
  await card({ open: true, report: report({ body: 'Cosine decay is done.\n\n<run id="r1" figure/>' }) }, figureRoutes(statsOnly));
  await settle();
  assert.ok(opened().textContent.includes("Cosine decay is done."));
  assert.equal(opened().querySelector(".border"), null);
});

test("a figure of a run without a parent has no Logs chip, and its experiment's name opens the experiment", async () => {
  const state = { open: true, report: report({ body: 'Cosine decay is running.\n\n<run id="r1" figure/>' }), opened: [] };
  await card(state, figureRoutes(running));
  await settle();
  const doc = opened();
  assert.equal(doc.querySelector(".run-chip"), null, "no Logs chip");
  assert.ok(!doc.textContent.includes(m.tree_view_logs()));
  const title = find(".md h3 button", "Cosine decay");
  assert.ok(title, "the chart's title names the experiment");
  await click(title);
  assert.deepEqual(state.opened, [["r1", "preview"]]);
});

test("a figure of a run with a parent draws both on one chart of the lead signal, each legend entry opening its experiment", async () => {
  const points = running.series.loss.map(([x, y]) => [x, y === null ? null : y * 1.1]);
  const compared = { ...running, headline: "loss", parent: { runId: "r0", phase: null, series: { loss: points } } };
  const state = { open: true, report: report({ body: 'Cosine decay beats the baseline.\n\n<run id="r1" figure/>' }), opened: [] };
  await card(state, figureRoutes(compared));
  await settle();
  const doc = opened();
  assert.equal(doc.querySelectorAll('svg[role="img"]').length, 1, "one chart");
  assert.equal(doc.querySelector(".run-chip"), null, "no Logs chip");
  const legend = [...doc.querySelectorAll(".md li button")].map((entry) => plain(entry.textContent));
  assert.deepEqual(legend.sort(), ["Baseline", "Cosine decay"]);
  await click(find(".md li button", "Baseline"));
  await click(find(".md li button", "Cosine decay"));
  assert.deepEqual(state.opened, [["r0", "preview"], ["r1", "preview"]]);
});
test("a failed update says why with Retry, and a refused retry shows the server's message", async () => {
  const state = { report: report({ error: { message: "Claude Haiku 4.5 is out of credits.", at: now } }) };
  const { requests } = await card(state);
  assert.ok(find('[role="alert"]', "Claude Haiku 4.5 is out of credits."), "the error shows on the folded card");

  state.refuse = { "POST /api/chat/sessions/s/report/refresh": { status: 400, error: "Run watching is off. Turn it on in Settings." } };
  await click(find("button", m.app_retry()));
  assert.equal(requests.filter((r) => r.request === "POST /api/chat/sessions/s/report/refresh").length, 1);
  assert.ok(find('[role="alert"]', "Run watching is off. Turn it on in Settings."));

  state.refuse = {};
  await click(find("button", m.app_retry()));
  assert.equal(find('[role="alert"]'), null, "the server cleared the failure");
  assert.equal(find("button", m.app_retry()), null);
  assert.equal(section().querySelector(".animate-spin, [role=progressbar]"), null, "no spinner before the update starts");
  assert.ok(!find('[role="status"]', m.status_report_updating()));
});

test("a refused retry stays only until the server reports the status again", async () => {
  const state = { report: report({ error: { message: "Claude Haiku 4.5 is out of credits.", at: now } }), refuse: { "POST /api/chat/sessions/s/report/refresh": { status: 409, error: "Run watching is off. Turn it on in Settings." } } };
  await card(state);
  await click(find("button", m.app_retry()));
  assert.ok(find('[role="alert"]', "Run watching is off. Turn it on in Settings."));

  await emit({ sessionId: "s", report: report({ error: { message: "Claude Haiku 4.5 is still out of credits.", at: now + 1 } }) });
  assert.equal(find('[role="alert"]', "Run watching is off. Turn it on in Settings."), null);
  assert.ok(find('[role="alert"]', "Claude Haiku 4.5 is still out of credits."));
});

test("before the first write the card says it is writing, and shows nothing once there is no status", async () => {
  await card({ report: report({ body: null, updatedAt: null, writing: true }) });
  assert.ok(section().textContent.includes(m.status_report_first_writing()));
  assert.equal(button(m.common_show_more()), null, "nothing to open yet");
});

test("an older server, which has no status, shows no card and no error", async () => {
  serve((path) => path === "/api/settings/ui-state" ? { onboardingCompleted: true } : path === "/api/chat/sessions/s/report" ? new Response("", { status: 404 }) : undefined);
  await render(createElement(StatusReport, { sessionId: "s", onTurnOff() {} }));
  await settle();
  assert.equal(section(), null);
});

test("a chat with no milestone yet shows no card", async () => {
  await card({ report: null });
  assert.equal(section(), null);
});

test("a status that fails to load says so with Retry instead of hiding", async () => {
  let failing = true;
  serve((path) => {
    if (path === "/api/settings/ui-state") return { onboardingCompleted: true };
    if (path === "/api/chat/sessions/s/report") return failing ? Response.json({ error: "database is locked" }, { status: 500 }) : report();
  });
  const workspace = createWorkspace("test");
  workspace.queryClient.setDefaultOptions({ queries: { retry: false } });
  await render(createElement(StatusReport, { sessionId: "s", onTurnOff() {} }), workspace);
  await settle();
  assert.ok(find('[role="alert"]', m.status_report_load_failed()));
  failing = false;
  await click(find("button", m.app_retry()));
  assert.ok(section().textContent.includes("Comparing the best two learning rates."));
});

test("a chat.report event rewrites an open card in place", async () => {
  const state = { open: true, report: report() };
  await card(state);

  await emit({ sessionId: "s", report: report({ writing: true }) });
  assert.ok(find('[role="status"]', m.status_report_updating()));

  await emit({ sessionId: "s", report: report({ body: `Seeds agree on 3e-4.\n\n- Three seeds within 0.01 loss.`, updatedAt: Date.now() }) });
  assert.ok(opened().textContent.includes("Three seeds within 0.01 loss."));
  assert.ok(find('[role="status"]', m.status_report_updated()), "the finished update is announced");

  await emit({ sessionId: "other", report: report({ body: "Another chat's status." }) });
  assert.ok(!section().textContent.includes("Another chat's status."));

  await emit({ sessionId: "s", report: null });
  assert.equal(section(), null, "a chat whose status is gone shows no card");
});

test("a failed update is announced as updating, never as updated", async () => {
  const state = { report: report() };
  await card(state);
  assert.ok(!find('[role="status"]', m.status_report_updated()), "loading the status announces nothing");

  await emit({ sessionId: "s", report: report({ writing: true }) });
  await emit({ sessionId: "s", report: report({ error: { message: "Claude Haiku 4.5 is out of credits.", at: now } }) });
  assert.ok(!find('[role="status"]', m.status_report_updated()));

  await emit({ sessionId: "s", report: report({ writing: true }) });
  await emit({ sessionId: "s", report: report({ updatedAt: now }) });
  assert.ok(find('[role="status"]', m.status_report_updated()));
});

test("a gap in the event stream reads the status again", async () => {
  const state = { report: report() };
  const { requests } = await card(state);
  state.report = report({ body: "Written while the page was away." });
  await act(async () => streams[0].dispatchEvent(new MessageEvent("resync.required", { data: "{}" })));
  await settle();
  assert.equal(requests.filter((r) => r.request === "GET /api/chat/sessions/s/report").length, 2);
  assert.ok(section().textContent.includes("Written while the page was away."));
});

test("a chat whose status the user turned off shows no card", async () => {
  await card({ report: report({ enabled: false }) });
  assert.equal(section(), null);
});

test("the card's menu turns the status off for this chat at once and says how to get it back", async () => {
  const state = { report: report() };
  const { requests } = await card(state);
  await click(button(m.status_report_options()));
  await click(find('[role="menuitem"]', m.status_report_turn_off()));
  assert.deepEqual(requests.find((r) => r.request === "POST /api/chat/sessions/s/report/enabled").body, { enabled: false });
  assert.equal(section(), null);
  assert.ok(document.body.textContent.includes(m.status_report_turned_off()));
});

test("a refused turn-off brings the card back with the server's reason", async () => {
  const state = { report: report(), refuse: { "POST /api/chat/sessions/s/report/enabled": { status: 409, error: "Run watching is off. Turn it on in Settings." } } };
  await card(state);
  await click(button(m.status_report_options()));
  await click(find('[role="menuitem"]', m.status_report_turn_off()));
  assert.ok(section(), "the card is back");
  assert.ok(document.body.textContent.includes("Run watching is off. Turn it on in Settings."));
});

/** The chat's row menu in the sidebar, over a server whose status for chat s is `held.report`. */
async function rowMenu(held, sessionFields = {}) {
  const chat = await renderChat("/projects/p/tasks/s", {
    sessions: [session("s", sessionFields)],
    routes: {
      "GET /api/chat/sessions/s/report": () => held.report,
      "POST /api/chat/sessions/s/report/enabled": (init) => {
        held.report = { ...(held.report ?? report({ body: null, updatedAt: null })), enabled: JSON.parse(init.body).enabled };
        return held.report;
      },
    },
  });
  const row = [...document.querySelectorAll(".session-row")].find((element) => element.textContent.includes("Chat s"));
  await act(async () => row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 })));
  await settle();
  return { ...chat, item: () => find('[role="menuitem"]', m.status_report_turn_on()) };
}

test("hovering a chat's row reads its status, so the menu's status item is there as it opens", { timeout: 20_000 }, async () => {
  const sessions = [session("s"), session("t")];
  const { requests } = await renderChat("/projects/p/tasks/s", { sessions, routes: { "GET /api/chat/sessions": () => ({ sessions }) } });
  const reads = () => requests.filter((r) => r.request === "GET /api/chat/sessions/t/report").length;
  const row = [...document.querySelectorAll(".session-row")].find((element) => element.textContent.includes("Chat t"));
  assert.equal(reads(), 0);
  await act(async () => row.dispatchEvent(new MouseEvent("mouseover", { bubbles: true })));
  await settle();
  assert.equal(reads(), 1, "read before the menu opens");
  await act(async () => row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 })));
  assert.ok(find('[role="menuitem"]', m.status_report_turn_on()));
  assert.equal(reads(), 1);
});

test("the chat's menu turns a status that was turned off back on", { timeout: 20_000 }, async () => {
  const held = { report: report({ enabled: false }) };
  const { item, requests } = await rowMenu(held);
  assert.ok(item());
  await click(item());
  assert.deepEqual(requests.find((r) => r.request === "POST /api/chat/sessions/s/report/enabled").body, { enabled: true });
  await settle();
  assert.ok(section(), "the card shows again");
});

test("the chat's menu offers to turn on a status the chat hasn't started yet", { timeout: 20_000 }, async () => {
  assert.ok((await rowMenu({ report: null })).item(), "no status yet");
});

test("the chat's menu has no status item while the status is on", { timeout: 20_000 }, async () => {
  assert.equal((await rowMenu({ report: report() })).item(), null);
});

test("a side chat's menu never offers a status", { timeout: 20_000 }, async () => {
  assert.equal((await rowMenu({ report: null }, { sideParentSessionId: "parent" })).item(), null);
});

test("Escape closes the card's menu without folding the card", async () => {
  await card({ open: true, report: report() });
  await click(button(m.status_report_options()));
  find('[role="menuitem"]', m.status_report_turn_off()).focus();
  await press("Escape");
  await settle();
  assert.equal(find('[role="menuitem"]', m.status_report_turn_off()), null);
  assert.ok(opened(), "the card stays open");
});

test("turning the status off in a chat moves focus to the composer", { timeout: 20_000 }, async () => {
  const held = { report: report() };
  await renderChat("/projects/p/tasks/s", {
    routes: {
      "GET /api/chat/sessions/s/report": () => held.report,
      "POST /api/chat/sessions/s/report/enabled": (init) => (held.report = { ...held.report, enabled: JSON.parse(init.body).enabled }),
    },
  });
  await click(button(m.status_report_options()));
  await click(find('[role="menuitem"]', m.status_report_turn_off()));
  await act(async () => new Promise((resolve) => requestAnimationFrame(resolve)));
  await settle();
  assert.equal(section(), null);
  assert.ok(composer().field().contains(document.activeElement), `focus is on ${document.activeElement?.outerHTML.slice(0, 80)}`);
});

test("the card's menu turns the status off for new chats, leaving this one", async () => {
  const state = { report: report(), uiState: { runWatching: true, statusForNewChats: true } };
  const { requests } = await card(state);
  await click(button(m.status_report_options()));
  await click(find('[role="menuitem"]', m.status_report_turn_off_new()));
  assert.deepEqual(requests.find((r) => r.request === "POST /api/settings/ui-state").body, { statusForNewChats: false });
  assert.ok(section(), "this chat keeps its status");
  assert.ok(document.body.textContent.includes(m.status_report_turned_off_new()));

  await click(button(m.status_report_options()));
  assert.equal(find('[role="menuitem"]', m.status_report_turn_off_new()), null, "already off, so Settings is where it turns back on");
});

/** The Settings rows over a server whose ui state is `uiState`; every write is kept. */
async function settingsRows(uiState) {
  const writes = [];
  serve((path, init) => {
    if (path !== "/api/settings/ui-state") return undefined;
    if (init.method === "POST") {
      writes.push(JSON.parse(init.body));
      return { onboardingCompleted: true, ...uiState, ...writes.at(-1) };
    }
    return { onboardingCompleted: true, ...uiState };
  });
  await render(createElement(RunWatchingRow));
  await settle();
  return { writes, toggle: () => document.querySelector(`[role="switch"][aria-label="${m.settings_status_new_chats()}"]`) };
}

test("Settings turns the status for new chats off and on beside run watching", async () => {
  const { writes, toggle } = await settingsRows({ runWatching: true, statusForNewChats: true });
  assert.ok(document.querySelector(`[role="switch"][aria-label="${m.settings_watch_runs()}"]`));
  assert.equal(toggle().getAttribute("aria-checked"), "true");
  await click(toggle());
  assert.deepEqual(writes, [{ statusForNewChats: false }]);
  assert.equal(toggle().getAttribute("aria-checked"), "false");
});

test("with run watching off, the status switch is off, disabled, and says why", async () => {
  const { toggle } = await settingsRows({ runWatching: false, statusForNewChats: true });
  assert.equal(toggle().getAttribute("aria-checked"), "false");
  assert.ok(toggle().disabled);
  assert.ok(document.body.textContent.includes(m.settings_status_new_chats_needs_watching({ setting: m.settings_watch_runs() })), "names the switch it needs");
});

test("a server without the setting shows no status switch", async () => {
  const { toggle } = await settingsRows({ runWatching: true });
  assert.equal(toggle(), null);
});

const offered = (same) => report({ body: null, updatedAt: null, needsModel: { same } });
const claude = { ...codex, id: "claude-code", name: "Claude Code", models: [model("opus", "Claude Opus 5.5"), model("haiku", "Claude Haiku 4.5")], defaultModel: "opus" };
const codexWithLuna = { ...codex, models: [...codex.models, model("gpt-6-luna", "GPT-6 Luna")] };
const haiku = { harness: "claude-code", profile: null, model: "haiku" };
const luna = { harness: "codex", profile: null, model: "gpt-6-luna" };
const pickerRows = () => [...document.querySelectorAll(".model-menu [data-row] [data-nav]")].map((row) => plain(row.textContent));
const statusButton = (text) => [...section().querySelectorAll("button")].find((element) => plain(element.textContent) === text) ?? null;

/** Chat s open over a server whose status for it is `held.report`, written by one of `held.models`; `held.refuse` refuses a pick with that error. */
async function offer(held) {
  const chat = await renderChat("/projects/p/tasks/s", {
    harnesses: [claude, codexWithLuna],
    routes: {
      "GET /api/chat/sessions/s/report": () => held.report,
      "GET /api/chat/sessions/s/report/models": () => ({ models: held.models }),
      "POST /api/chat/sessions/s/report/model": () => held.refuse ? refused(held.refuse) : (held.report = { ...held.report, needsModel: null, writing: true }),
    },
  });
  return { ...chat, picked: () => chat.requests.find((r) => r.request === "POST /api/chat/sessions/s/report/model").body };
}

test("a chat whose status has no model offers to enable it with the chat's own model", { timeout: 20_000 }, async () => {
  const { picked } = await offer({ report: offered(true), models: [haiku, luna] });
  assert.ok(section().textContent.includes(m.status_report_offer()));
  assert.ok(button(m.status_report_options()), "the menu that turns it off stays");
  await click(statusButton(m.status_report_enable()));
  assert.deepEqual(picked(), { same: true });
  assert.ok(section().textContent.includes(m.status_report_first_writing()), "it starts writing at once");
});

test("the chevron beside Enable opens the model picker with only the models that can write a status", { timeout: 20_000 }, async () => {
  const { picked } = await offer({ report: offered(true), models: [haiku, luna] });
  await click(button(m.status_report_other_model()));
  assert.deepEqual(pickerRows().sort(), ["Claude Haiku 4.5", "GPT-6 Luna"]);
  await click(find(".model-menu [data-row] [data-nav]", "GPT-6 Luna"));
  assert.deepEqual(picked(), luna);
});

test("without a usable chat model, the one button opens the model picker", { timeout: 20_000 }, async () => {
  const { picked } = await offer({ report: offered(false), models: [haiku, luna] });
  assert.equal(statusButton(m.status_report_enable()), null);
  await click(statusButton(m.status_report_choose_model()));
  await click(find(".model-menu [data-row] [data-nav]", "Claude Haiku 4.5"));
  assert.deepEqual(picked(), haiku);
});

test("with no model that can write a status, the offer opens Settings to set one up", { timeout: 20_000 }, async () => {
  await offer({ report: offered(false), models: [] });
  assert.ok(section().textContent.includes(m.status_report_no_models()));
  assert.equal(statusButton(m.status_report_choose_model()), null);
  await click(statusButton(m.status_report_set_up_agents()));
  assert.equal(location.pathname, "/projects/p/settings/agents", "Settings → Agents is open");
});

test("models the server can't list yet show its reason with Retry", { timeout: 20_000 }, async () => {
  let reading = true;
  const busy = "orx is still reading which models your agents offer. Try again in a moment.";
  await renderChat("/projects/p/tasks/s", {
    harnesses: [claude, codexWithLuna],
    routes: {
      "GET /api/chat/sessions/s/report": () => offered(false),
      "GET /api/chat/sessions/s/report/models": () => reading ? Response.json({ error: busy }, { status: 503 }) : { models: [haiku] },
    },
  });
  assert.ok(find('[role="alert"]', busy));
  reading = false;
  await click(statusButton(m.app_retry()));
  assert.equal(find('[role="alert"]'), null);
  assert.ok(statusButton(m.status_report_choose_model()));
});

test("using the offer on a status with a body leaves the card folded", { timeout: 20_000 }, async () => {
  await offer({ report: report({ needsModel: { same: true } }), models: [haiku, luna] });
  const toggle = () => button(m.common_show_more());
  await click(button(m.status_report_other_model()));
  assert.ok(pickerRows().length, "the picker is open");
  assert.equal(toggle().getAttribute("aria-expanded"), "false", "opening the picker leaves the card folded");
  await press("Escape");
  await settle();
  await click(statusButton(m.status_report_enable()));
  assert.equal(toggle().getAttribute("aria-expanded"), "false");
  assert.equal(opened(), null);
});

test("a refused pick brings the offer back with the server's reason", { timeout: 20_000 }, async () => {
  await offer({ report: offered(true), models: [haiku], refuse: "Claude Sonnet 5.5 is signed out. Sign in from Settings." });
  await click(statusButton(m.status_report_enable()));
  assert.ok(find('[role="alert"]', "Claude Sonnet 5.5 is signed out. Sign in from Settings."));
  assert.ok(statusButton(m.status_report_enable()), "the offer is back");
  assert.equal(statusButton(m.app_retry()), null, "the offer is the way to try again");
});

test("a refused pick clears once another window picks a model", { timeout: 20_000 }, async () => {
  const held = { report: offered(true), models: [haiku], refuse: "Claude Sonnet 5.5 is signed out. Sign in from Settings." };
  await offer(held);
  await click(statusButton(m.status_report_enable()));
  assert.ok(find('[role="alert"]', held.refuse));
  await emit({ sessionId: "s", report: report({ body: null, updatedAt: null, writing: true }) });
  assert.equal(find('[role="alert"]'), null);
  assert.ok(section().textContent.includes(m.status_report_first_writing()));
});

import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { ProjectTerminal } from "../src/components/ProjectTerminal.tsx";
import * as tabs from "../src/workspaceTabs.ts";
import { m } from "../src/paraglide/messages.js";
import { find, render, renderApp, serveApp, settle } from "./render.mjs";

const terminal = { kind: "home", view: "terminal" };

test("the terminal tab keeps its shell's id across a reload and drops it once closed", () => {
  const opened = tabs.applyPane(tabs.initialRightPaneSessionState(), terminal);
  assert.match(opened.terminalId, /^[0-9a-f-]{36}$/);
  const saved = tabs.rememberWorkspace({ ...opened, rightTab: "terminal", panelOpen: true }, {}, {});
  assert.equal(saved.terminalId, opened.terminalId);
  assert.equal(tabs.restoreWorkspace(saved, undefined).terminalId, opened.terminalId);
  assert.equal(tabs.restoreWorkspace(saved, terminal).terminalId, opened.terminalId);

  const older = { ...saved };
  delete older.terminalId;
  assert.match(tabs.restoreWorkspace(older, undefined).terminalId, /^[0-9a-f-]{36}$/);

  const closed = tabs.rememberWorkspace({ ...opened, terminalTabOpen: false }, {}, {});
  assert.equal("terminalId" in closed, false);
  assert.equal(tabs.restoreWorkspace(closed, undefined).terminalId, null);
});

/** A page's sockets to the server, each driven by the test. */
function sockets() {
  const opened = [];
  globalThis.WebSocket = class extends EventTarget {
    static OPEN = 1;
    readyState = 0;
    sent = [];
    constructor(url) { super(); this.url = new URL(String(url)); opened.push(this); }
    send(data) { this.sent.push(data); }
    close() { this.readyState = 3; }
    open() { this.readyState = 1; this.onopen?.(); }
    drop() { this.readyState = 3; this.onclose?.(); }
    say(value) { this.onmessage?.({ data: JSON.stringify(value) }); }
  };
  return opened;
}

const status = (container) => container.querySelector('[role="status"]').textContent;
const button = (container, label) => [...container.querySelectorAll("button")].find((item) => item.textContent === label);

async function shell(props = {}) {
  const opened = sockets();
  let restarts = 0;
  const view = await render(createElement(ProjectTerminal, {
    projectId: "project", sessionId: "chat", terminalId: "tab-1", active: true, onRestart: () => restarts++, ...props,
  }));
  await settle();
  return { ...view, opened, get restarts() { return restarts; } };
}

test("a lost connection reconnects to the same shell, quietly at first", async (t) => {
  const { container, opened } = await shell();
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const wait = (ms) => act(async () => t.mock.timers.tick(ms));
  assert.equal(opened.length, 1);
  assert.equal(opened[0].url.pathname, "/api/projects/project/terminal");
  assert.equal(opened[0].url.searchParams.get("terminalId"), "tab-1");
  assert.equal(opened[0].url.searchParams.get("sessionId"), "chat");
  await act(() => opened[0].open());
  assert.equal(JSON.parse(opened[0].sent[0]).type, "resize");

  await act(() => opened[0].drop());
  assert.equal(status(container), "");
  await wait(300);
  assert.equal(opened.length, 2);
  assert.equal(opened[1].url.href, opened[0].url.href);
  await act(() => opened[1].drop());
  await wait(800);
  assert.equal(status(container), m.workspace_terminal_reconnecting());
  assert.equal(button(container, m.workspace_terminal_restart()), undefined);

  await wait(200);
  const latest = opened.at(-1);
  await act(() => latest.open());
  assert.equal(status(container), "");
  assert.equal(JSON.parse(latest.sent[0]).type, "resize");
});

test("a shell that is gone says so and offers a new one", async (t) => {
  const view = await shell();
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const [socket] = view.opened;
  await act(() => { socket.open(); socket.say({ type: "gone" }); socket.drop(); });
  await settle();
  assert.equal(status(view.container), m.workspace_terminal_gone());
  await act(async () => t.mock.timers.tick(300));
  assert.equal(view.opened.length, 1);
  await act(() => button(view.container, m.workspace_terminal_restart()).click());
  assert.equal(view.restarts, 1);
});

test("a shell shown in another window can be shown here again", async () => {
  const view = await shell();
  const [socket] = view.opened;
  await act(() => { socket.open(); socket.say({ type: "elsewhere" }); socket.drop(); });
  await settle();
  assert.equal(status(view.container), m.workspace_terminal_elsewhere());
  await act(() => button(view.container, m.workspace_terminal_show_here()).click());
  await settle();
  assert.equal(view.opened.length, 2);
  assert.equal(view.opened[1].url.searchParams.get("terminalId"), "tab-1");
  assert.equal(view.restarts, 0);
});

test("an exited shell reports its code and offers a new one", async () => {
  const view = await shell();
  const [socket] = view.opened;
  await act(() => { socket.open(); socket.say({ type: "exit", code: 3 }); socket.drop(); });
  await settle();
  assert.equal(status(view.container), m.workspace_terminal_exited({ code: "3" }));
  await act(() => button(view.container, m.workspace_terminal_restart()).click());
  assert.equal(view.restarts, 1);
});

test("the terminal tab saves its shell's id and closing the tab ends that shell", async () => {
  const opened = sockets();
  const saved = [], deleted = [];
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }),
    "POST /api/projects/p/ui-state": (init) => { const body = JSON.parse(init.body); saved.push(body); return body; },
  });
  const app = globalThis.fetch;
  globalThis.fetch = (input, init = {}) => {
    if (init.method !== "DELETE") return app(input, init);
    deleted.push(new URL(String(input), location.href).pathname);
    return Promise.resolve(Response.json({ ok: true }));
  };
  await renderApp(`/projects/p/tasks/new?pane=${encodeURIComponent(JSON.stringify(terminal))}`);
  await settle();
  const id = opened.at(-1).url.searchParams.get("terminalId");
  assert.match(id, /^[0-9a-f-]{36}$/);
  assert.equal(saved.at(-1).tasks.new.terminalId, id);

  const close = find(`button:has(> .tab-label[data-label="${m.workspace_terminal()}"]) .tab-close`);
  await act(async () => close.click());
  await settle();
  assert.deepEqual(deleted, [`/api/projects/p/terminals/${id}`]);
  assert.equal("terminalId" in saved.at(-1).tasks.new, false);
});

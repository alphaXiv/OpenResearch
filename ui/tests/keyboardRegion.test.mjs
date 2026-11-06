import assert from "node:assert/strict";
import { test } from "node:test";
import { act } from "react";
import { commandRegion, subscribeCommandRegion } from "../src/keyboardRegion.ts";
import { navigate, renderApp, serveApp, settle } from "./render.mjs";

// The terminal tab's shell never connects.
globalThis.WebSocket = class extends EventTarget { send() {} close() {} };
// Wide enough for the sidebar beside an open panel.
happyDOM.setViewport({ width: 1600, height: 900 });

/** A focusable element inside one container per selector, outermost first: `.class` or `[role=name]`. */
function within(...selectors) {
  let parent = document.body;
  for (const selector of selectors) {
    const element = parent.appendChild(document.createElement("div"));
    const role = selector.match(/^\[role=(\w+)\]$/)?.[1];
    if (role) element.setAttribute("role", role);
    else element.className = selector.slice(1);
    parent = element;
  }
  return parent.appendChild(document.createElement("button"));
}
const click = (target) => target.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));

test("focus chooses the left session or right tab, including embedded side chats", () => {
  assert.equal(commandRegion(within(".session-rail")), "left");
  assert.equal(commandRegion(within(".chat-pane")), "left");
  assert.equal(commandRegion(within(".right-pane", ".chat-pane")), "right");
  assert.equal(commandRegion(within()), null);
});

test("body focus falls back to the last clicked pane, including native menu commands", () => {
  click(within(".right-pane"));
  document.activeElement?.blur();
  assert.equal(commandRegion(), "right");
  click(within(".session-rail"));
  assert.equal(commandRegion(), "left");
  assert.equal(commandRegion(within(".right-pane")), "right");
  click(within());
  assert.equal(commandRegion(), null);
});

test("the Windows menu preserves its originating pane", () => {
  click(within(".right-pane"));
  click(within("[role=menubar]"));
  assert.equal(commandRegion(within("[role=menu]")), "right");
});

/** A project with chats `s` and `side` and an experiment `exp` on branch `b`, open at `path`. */
async function project(path) {
  const chat = (id) => ({ id, projectId: "p", harness: "claude", title: id, model: null, serviceTier: null, permissionMode: null, planMode: false, autonomy: "default", reasoningLevel: null, archived: false, createdAt: 1, updatedAt: 1, busy: false, activeLeafId: null });
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }),
    "GET /api/projects/p/experiments": () => ({ experiments: [{ id: "exp", projectId: "p", slug: "exp", branchName: "b", runCommand: "", agentStatus: "", createdAt: 1, updatedAt: 1, archived: false }] }),
    "GET /api/chat/sessions": () => ({ sessions: [chat("s"), chat("side")] }),
  });
  await renderApp(path);
  return { go: navigate, pane: () => JSON.parse(new URLSearchParams(location.search).get("pane")), tabs: () => document.querySelectorAll(".right-pane .tab-close").length };
}

const withPane = (pane) => `/projects/p/tasks/s?pane=${encodeURIComponent(JSON.stringify(pane))}`;
const files = { kind: "home", view: "files" };
const focus = async (element) => {
  await act(async () => element.focus());
  await settle();
};
const closeTab = async () => {
  await act(async () => window.dispatchEvent(new CustomEvent("orx:command", { detail: "tab.close" })));
  await settle();
};

for (const pane of [
  files, { kind: "home", view: "terminal" }, { kind: "home", view: "artifacts" }, { kind: "home", view: "experiments" },
  { kind: "code", experimentId: "exp", branch: "b", view: "changes" }, { kind: "file", path: "paper.md" }, { kind: "experiment", experimentId: "exp", view: "overview" },
  { kind: "plan", sessionId: "s", promptId: "plan" }, { kind: "subagent", sessionId: "s", spawnPartId: "spawn" }, { kind: "side", sessionId: "side" },
]) {
  test(`Close Tab closes the focused right ${pane.view ?? pane.kind} tab and falls back to the previous one`, async () => {
    const previous = pane === files ? { kind: "home", view: "artifacts" } : files;
    const app = await project(withPane(previous));
    await app.go(withPane(pane));
    assert.equal(app.tabs(), 2);
    await focus(document.querySelector(".right-pane [data-workspace-content]"));
    await closeTab();
    assert.deepEqual(app.pane(), previous);
    assert.equal(app.tabs(), 1);
  });
}

test("Close Tab on the left deselects the chat without closing its right tabs", async () => {
  const app = await project(withPane(files));
  await focus(document.querySelector(".session-rail button"));
  await closeTab();
  assert.equal(location.pathname, "/projects/p/tasks/new");
  await act(async () => history.back());
  await settle();
  assert.deepEqual(app.pane(), files);
  assert.equal(app.tabs(), 1);
});

test("Close Tab on the left leaves settings, and does nothing on a new chat", async () => {
  const app = await project("/projects/p/settings/settings");
  await focus(document.querySelector(".session-rail button"));
  await closeTab();
  assert.match(location.pathname, /^\/projects\/p\/tasks\//);
  await app.go("/projects/p/tasks/new");
  await focus(document.querySelector(".session-rail button"));
  await closeTab();
  assert.equal(location.pathname + location.search, "/projects/p/tasks/new");
});

test("Close Tab does nothing outside a pane or while the right pane is closed", async () => {
  const app = await project(withPane(files));
  await focus(document.querySelector(".titlebar-sidebar-toggle button"));
  await closeTab();
  assert.deepEqual(app.pane(), files);
  assert.equal(app.tabs(), 1);

  click(document.querySelector(".right-pane [data-workspace-content]"));
  await app.go("/projects/p/tasks/s");
  document.activeElement?.blur();
  assert.equal(commandRegion(), "right");
  await closeTab();
  assert.equal(app.pane(), null);
});

test("keyboard entry into the Windows menu preserves the previously focused pane", async () => {
  click(within());
  const release = subscribeCommandRegion(() => {});
  within(".right-pane").focus();
  within("[role=menubar]").focus();
  await Promise.resolve();
  assert.equal(commandRegion(), "right");
  release();
});

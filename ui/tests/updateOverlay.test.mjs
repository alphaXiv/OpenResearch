import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { act, createElement } from "react";
import { holdUpdateBanner, ServerStopped, UpdateBanner, UpdateHandoff } from "../src/components/UpdateBanner.tsx";
import { createWorkspace } from "../src/workspace.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, render, renderApp, serve, serveApp, settle } from "./render.mjs";

let loads = 0;
/** Runs public/update-overlay.js as index.html does on each page load. */
const loadPage = () => import(`../public/update-overlay.js?load=${++loads}`);
const overlay = () => document.querySelector('[role="alertdialog"]');
const record = { from: "1.0.0", to: "2.0.0", title: "Updating OpenResearch", body: "Applying an update takes 5–10 seconds." };
const status = (build) => ({ build, instance: `server-${build}`, restartRequired: false, current: build });

beforeEach(() => {
  document.body.insertAdjacentHTML("afterbegin", '<div id="root"></div>');
});

afterEach(() => {
  sessionStorage.clear();
  document.getElementById("orx-update")?.remove();
  document.getElementById("root")?.remove();
});

test("the next page draws the overlay the restarting page showed before anything else loads", async () => {
  await loadPage();
  window.orxUpdateOverlay.begin(record);
  assert.equal(overlay().getAttribute("aria-busy"), "true");
  assert.ok(document.getElementById("root").inert);

  document.getElementById("orx-update").remove();
  document.getElementById("root").inert = false;
  await loadPage();
  assert.equal(overlay().querySelector("h2").textContent, record.title);
  assert.ok(overlay().textContent.includes(record.body));
  assert.ok(document.getElementById("root").inert, "the page under it takes no input");
});

test("a page loaded without a restart, or long after one, shows nothing", async () => {
  await loadPage();
  assert.equal(overlay(), null);
  sessionStorage.setItem("orx:applying-update", JSON.stringify({ ...record, at: Date.now() - 60 * 60 * 1000 }));
  await loadPage();
  assert.equal(overlay(), null);
  assert.equal(sessionStorage.getItem("orx:applying-update"), null);
});

async function handoff(build) {
  await loadPage();
  window.orxUpdateOverlay.begin(record);
  serve((path) => (path === "/api/update" ? status(build) : undefined));
  const workspace = createWorkspace("test");
  const view = await render(createElement(UpdateHandoff, { workspace }), workspace);
  await settle();
  return view;
}

test("the overlay goes once the new build answers", async () => {
  await handoff("2.0.0");
  assert.ok(document.getElementById("orx-update").hasAttribute("data-leaving"));
  assert.equal(window.orxUpdateOverlay.pending(), null);
  assert.equal(document.getElementById("root").inert, false);
});

test("a restart that came back on the same build says the update did not install", async () => {
  await handoff("1.0.0");
  assert.equal(overlay().querySelector("h2").textContent, m.update_overlay_unchanged_title());
  assert.equal(overlay().getAttribute("aria-busy"), "false");
  const close = find("button", m.update_overlay_close());
  assert.equal(document.activeElement, close);
  await act(() => close.click());
  assert.ok(document.getElementById("orx-update").hasAttribute("data-leaving"));
  assert.equal(find("button", m.update_overlay_close()), null);
  assert.equal(window.orxUpdateOverlay.pending(), null);
});

test("every page reloads once the server runs another build", async (t) => {
  await loadPage();
  const reload = t.mock.method(window.location, "reload", () => {});
  serve((path) => (path === "/api/update" ? status("1.0.0") : undefined));
  const workspace = createWorkspace("test");
  await render(createElement(UpdateHandoff, { workspace }), workspace);
  await settle();
  const key = workspace.queries.getUpdateStatusQuery().queryKey;
  await act(() => workspace.setScopedQueryData(key, { ...status("1.0.0"), instance: "restarted" }));
  await settle();
  assert.equal(reload.mock.callCount(), 0, "a restart on the same build keeps the page");
  await act(() => workspace.setScopedQueryData(key, status("2.0.0")));
  await settle();
  assert.equal(reload.mock.callCount(), 1);
});

test("a desktop page whose server stopped says why, and offers to start it again or quit", async (t) => {
  await loadPage();
  const sent = [];
  window.ipc = { postMessage: (message) => sent.push(message) };
  t.after(() => delete window.ipc);
  const reload = t.mock.method(window.location, "reload", () => {});
  let up = false;
  serve((path) => (path === "/api/update" && up ? status("1.0.0") : undefined));
  const workspace = createWorkspace("test");
  await render(createElement(ServerStopped, { workspace }), workspace);

  const reason = "Another program is using port 4792. Quit it and try again.";
  const log = "orx up: starting\nexit status: 1";
  await act(() => window.dispatchEvent(new CustomEvent("orx:server-stopped", { detail: { reason, log } })));
  assert.equal(overlay().querySelector("h2").textContent, m.server_stopped_title());
  assert.equal(document.getElementById(overlay().getAttribute("aria-describedby")).textContent, reason);
  assert.equal(find("summary", m.server_stopped_details()).parentElement.querySelector("pre").textContent, log);
  assert.equal(document.activeElement, find("button", m.server_stopped_restart()));
  await act(() => find("button", m.server_stopped_quit()).click());
  assert.deepEqual(sent, ["server:asking", "app:quit"]);

  t.mock.timers.enable({ apis: ["setTimeout"] });
  await act(() => find("button", m.server_stopped_restart()).click());
  assert.deepEqual(sent, ["server:asking", "app:quit", "server:start"]);
  assert.equal(overlay().getAttribute("aria-busy"), "true");
  assert.equal(find("button", m.server_stopped_restart()), null);
  assert.equal(find("summary", m.server_stopped_details()), null);
  await act(async () => t.mock.timers.tick(500));
  await settle();
  assert.equal(reload.mock.callCount(), 0, "the page waits for the server it started");
  up = true;
  await act(async () => t.mock.timers.tick(500));
  await settle();
  assert.equal(reload.mock.callCount(), 1);
});

test("a server that stopped without a reason, or under a window that sends none, offers the restart alone", async () => {
  await loadPage();
  const workspace = createWorkspace("test");
  await render(createElement(ServerStopped, { workspace }), workspace);

  await act(() => window.dispatchEvent(new CustomEvent("orx:server-stopped")));
  assert.ok(overlay().textContent.includes(m.server_stopped_body()));
  assert.equal(find("summary", m.server_stopped_details()), null);

  await act(() => window.dispatchEvent(new CustomEvent("orx:server-stopped", { detail: { reason: null, log: "signal: 9 (SIGKILL)" } })));
  assert.ok(overlay().textContent.includes(m.server_stopped_body()));
  assert.ok(find("summary", m.server_stopped_details()));
});

test("a server that stops under the update's notice leaves only its own", async () => {
  await handoff("1.0.0");
  assert.ok(find("button", m.update_overlay_close()));
  const workspace = createWorkspace("test");
  await render(createElement(ServerStopped, { workspace }), workspace);

  await act(() => window.dispatchEvent(new CustomEvent("orx:server-stopped", { detail: { reason: null, log: "" } })));
  assert.equal(overlay().querySelector("h2").textContent, m.server_stopped_title());
  assert.equal(find("button", m.update_overlay_close()), null);
  assert.ok(find("button", m.server_stopped_restart()));
});

test("a server that stops while an update waits on it keeps its notice past the wait", async (t) => {
  await loadPage();
  serveApp({
    "GET /api/pending-work": () => ({ agents: [], runs: [] }),
    "GET /api/update": () => ({ ...status("1.0.0"), installedVersion: "2.0.0", restartRequired: true, canRestart: true }),
    "POST /api/update/restart": () => ({}),
  });
  await renderApp("/");
  await render(createElement(ServerStopped, { workspace: createWorkspace("test") }));

  t.mock.timers.enable({ apis: ["setTimeout", "Date"] });
  await act(async () => find("button", m.update_banner_restart()).click());
  await settle();
  await act(() => window.dispatchEvent(new CustomEvent("orx:server-stopped", { detail: { reason: null, log: "" } })));
  await act(async () => t.mock.timers.tick(10 * 60 * 1000));
  await settle();
  assert.equal(overlay().querySelector("h2").textContent, m.server_stopped_title());
  assert.ok(!document.getElementById("orx-update").hasAttribute("data-leaving"));
  assert.ok(find("button", m.server_stopped_restart()));
});

test("on Windows the overlay draws the caption buttons it covers, and so does the next page before it loads", async (t) => {
  window.__ORX_WIN_TITLEBAR__ = true;
  const sent = [];
  window.ipc = { postMessage: (message) => sent.push(message) };
  t.after(() => {
    delete window.__ORX_WIN_TITLEBAR__;
    delete window.ipc;
    document.documentElement.removeAttribute("data-maximized");
  });
  await loadPage();
  const { WindowControls } = await import("../src/components/WindowControls.tsx");
  await render(createElement(WindowControls));
  const caption = (name) => document.querySelector(`#orx-update [aria-label="${name}"]`);

  window.orxUpdateOverlay.begin(record);
  await act(() => caption(m.window_close()).click());
  await act(() => caption(m.window_minimize()).click());
  assert.deepEqual(sent, ["titlebar:close", "titlebar:minimize"]);

  document.getElementById("orx-update").remove();
  await loadPage();
  assert.ok(caption(m.window_maximize()));
  document.documentElement.setAttribute("data-maximized", "");
  await settle();
  assert.equal(caption(m.window_maximize()), null);
  await act(() => caption(m.window_restore()).click());
  assert.deepEqual(sent, ["titlebar:close", "titlebar:minimize", "titlebar:zoom"]);
});

test("an update that lands while the version popup is open waits for it to close, and one already shown stays", async () => {
  serve(() => undefined);
  const ready = { ...status("1.0.0"), restartRequired: true, installedVersion: "2.0.0", installedTag: "v2.0.0", canRestart: true };
  const banner = () => document.querySelector(".update-banner");

  holdUpdateBanner(true);
  const view = await render(createElement(UpdateBanner, { status: ready }));
  assert.equal(banner(), null);

  await act(() => holdUpdateBanner(false));
  assert.ok(banner());

  await act(() => holdUpdateBanner(true));
  await view.rerender(createElement(UpdateBanner, { status: ready }));
  assert.ok(banner(), "opening the popup over a shown banner keeps it");

  await view.rerender(createElement(UpdateBanner, { status: { ...ready, installedVersion: "3.0.0", installedTag: "v3.0.0" } }));
  assert.ok(banner(), "a newer build replaces a shown banner in place");
  await act(() => holdUpdateBanner(false));
});

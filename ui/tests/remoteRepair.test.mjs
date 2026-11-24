import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { ltr } from "../src/i18n";
import { m } from "../src/paraglide/messages.js";
import { find, renderApp, serveApp, settle } from "./render.mjs";

const repairCommand = "chmod go-w '/fixture/bin'";
const sockets = [];
globalThis.WebSocket = class extends EventTarget {
  static OPEN = 1;
  constructor(url) { super(); this.url = String(url); sockets.push(this); }
  send() {}
  close() {}
};

/** Opens the app against a remote host whose last connection was refused until `repairCommand` runs. */
async function repairScreen() {
  const session = {
    id: "s", host: "fixture", user: "me", status: "disconnected", version: null, dashboardProtocol: null,
    gatewayUrl: "http://localhost:4910/", error: "Permissions refused", repairCommand, installPaths: null,
    uiPreferences: { theme: null, locale: null }, canStartNewHost: false,
  };
  serveApp({ "GET /_orx/runtime": () => ({ kind: "ssh", version: "test", buildChannel: "stable", dashboardProtocol: 1, session }) });
  const view = await renderApp("/projects");
  return { ...view, button: (label) => find("button", label) };
}

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};

test("copy reports success only after the clipboard write resolves", async (t) => {
  let resolve;
  const writes = t.mock.method(navigator.clipboard, "writeText", () => new Promise((done) => { resolve = done; }));
  const view = await repairScreen();
  t.mock.timers.enable({ apis: ["setTimeout"] });
  await click(view.button(m.remote_repair_copy()));
  assert.deepEqual(writes.mock.calls.map(({ arguments: [text] }) => text), [repairCommand]);
  assert.ok(view.button(m.remote_repair_copy()));

  await act(async () => resolve());
  assert.ok(view.button(m.remote_repair_copied()));
  await act(async () => t.mock.timers.runAll());
  assert.ok(view.button(m.remote_repair_copy()));
});

test("clipboard failure reports an error without claiming copied", async (t) => {
  t.mock.method(navigator.clipboard, "writeText", async () => { throw new Error("Clipboard denied"); });
  const view = await repairScreen();
  await click(view.button(m.remote_repair_copy()));
  assert.ok(view.button(m.remote_repair_copy()));
  assert.equal(view.button(m.remote_repair_copied()), null);
  assert.match(document.body.textContent, new RegExp(m.file_tree_clipboard_unavailable()));
});

test("a failed terminal can be opened again without reconnecting", async () => {
  const view = await repairScreen();
  await click(view.button(m.remote_repair_run()));
  const terminal = () => document.querySelector(`[aria-label="${m.remote_repair_terminal()}"]`);
  assert.ok(terminal());
  assert.equal(new URL(sockets.at(-1).url).pathname, "/_orx/ssh/repair");

  await act(async () => sockets.at(-1).onerror());
  await settle();
  assert.equal(terminal(), null);
  assert.match(document.body.textContent, new RegExp(m.settings_terminal_closed()));
  await click(view.button(m.remote_repair_run()));
  assert.ok(terminal());
});

test("an older running host offers an update without stopping its work", async () => {
  const session = {
    id: "s", host: "fixture", user: "me", status: "needsUpdate", version: "old", dashboardProtocol: 1,
    gatewayUrl: "http://localhost:4910/", error: null, repairCommand: null,
    installPaths: { binary: "/fixture/bin/orx", database: "/fixture/data", cache: "/fixture/cache" },
    uiPreferences: { theme: null, locale: null }, canStartNewHost: false,
  };
  serveApp({ "GET /_orx/runtime": () => ({ kind: "ssh", version: "test", buildChannel: "stable", dashboardProtocol: 2, session }) });
  await renderApp("/projects");
  assert.ok(find("button", m.remote_update()));
  assert.equal(find("button", m.remote_stop_host()), null);
});

test("the host restart is announced while its workspace reconnects", async () => {
  const session = {
    id: "s", host: "fixture", user: "me", status: "restarting", version: "new", dashboardProtocol: 2,
    gatewayUrl: "http://localhost:4910/", error: null, repairCommand: null, installPaths: null,
    uiPreferences: { theme: null, locale: null }, canStartNewHost: false,
  };
  serveApp({ "GET /_orx/runtime": () => ({ kind: "ssh", version: "test", buildChannel: "stable", dashboardProtocol: 2, session }) });
  await renderApp("/projects");
  assert.ok(find("h1", m.remote_restarting_title({ host: ltr("fixture") })));
  assert.ok(document.querySelector('[role="status"]'));
  assert.ok(find("button", m.remote_machine_connecting()));
  assert.equal(find("button", m.remote_start_new_host()), null);
});

import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { HarnessSetup } from "../src/components/HarnessSetupDialog.tsx";
import { m } from "../src/paraglide/messages.js";
import { render, settle } from "./render.mjs";

function fakeSockets() {
  const sockets = [];
  globalThis.WebSocket = class extends EventTarget {
    static OPEN = 1;
    readyState = 1;
    sent = [];
    constructor(url) { super(); this.url = String(url); sockets.push(this); }
    send(data) { this.sent.push(data); }
    close() { this.readyState = 3; }
  };
  return sockets;
}

const commands = { install: "", installUrl: null, managedFrom: "registry.npmjs.org", login: "opencode auth login", update: "opencode upgrade", requiresNpm: false };
const status = (container) => container.querySelector('[role="status"]')?.textContent;

async function frame(socket, value) {
  await act(() => socket.onmessage({ data: JSON.stringify(value) }));
  await settle();
}

test("an install queued behind another says so until it starts", async () => {
  const sockets = fakeSockets();
  const harness = { id: "opencode", name: "OpenCode", installed: false, installBroken: false, authState: "needsLogin", needsConfigRepair: false };
  const { container } = await render(createElement(HarnessSetup, { harness, commands }));
  const [socket] = sockets;
  assert.equal(new URL(socket.url).searchParams.get("action"), "install");

  await frame(socket, { type: "queue", waiting: true });
  assert.equal(status(container), m.harness_setup_queued());
  await frame(socket, { type: "queue", waiting: false });
  assert.equal(status(container), m.harness_setup_installing({ agent: "OpenCode" }));
});

test("an update queued behind another says so while its terminal stays folded away", async () => {
  const sockets = fakeSockets();
  const harness = { id: "opencode", name: "OpenCode", installed: true, installBroken: false, authState: "ready", needsConfigRepair: false, version: "1.19.5" };
  const { container } = await render(createElement(HarnessSetup, { harness, commands, action: "update" }));
  const run = [...container.querySelectorAll("button")].find((button) => button.textContent === m.harness_setup_approve_run());
  await act(async () => run.click());
  await settle();
  const [socket] = sockets;
  assert.equal(new URL(socket.url).searchParams.get("action"), "update");
  assert.equal(container.querySelector("button[aria-expanded]").getAttribute("aria-expanded"), "false");
  await act(() => socket.onopen());

  await frame(socket, { type: "queue", waiting: true });
  assert.equal(status(container), m.harness_setup_queued());
  assert.equal(container.querySelector("button[aria-expanded]").getAttribute("aria-expanded"), "false");
  await frame(socket, { type: "queue", waiting: false });
  assert.equal(status(container), m.harness_setup_updating({ agent: "OpenCode" }));
});

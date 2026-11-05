import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { CommandTerminal } from "../src/components/SshConnectTerminal.tsx";
import { render, settle } from "./render.mjs";

test("Connect shows the server diagnostic even after an authentication prompt", async () => {
  const sockets = [];
  globalThis.WebSocket = class extends EventTarget {
    static OPEN = 1;
    readyState = 1;
    sent = [];
    constructor(url) { super(); this.url = String(url); sockets.push(this); }
    send(data) { this.sent.push(data); }
    close() { this.readyState = 3; }
  };
  const { container } = await render(createElement(CommandTerminal, { path: "/connect", label: "Connect", onComplete: () => false }));
  const [socket] = sockets;
  assert.equal(new URL(socket.url).pathname, "/connect");

  const error = "SSH helper did not start. Python 3.8+ is required.";
  await act(() => {
    socket.onmessage({ data: new TextEncoder().encode("Password: ").buffer });
    socket.onmessage({ data: JSON.stringify({ type: "error", error }) });
  });
  await settle();
  assert.equal(container.querySelector('[role="alert"]').textContent, error);

  const keys = container.querySelector("textarea");
  await act(() => {
    keys.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true, cancelable: true }));
    keys.dispatchEvent(new InputEvent("input", { data: "a", inputType: "insertText", bubbles: true }));
  });
  assert.deepEqual(socket.sent, []);
});

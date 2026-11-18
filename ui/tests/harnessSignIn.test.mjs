import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { HarnessSetup } from "../src/components/HarnessSetupDialog.tsx";
import { render, settle } from "./render.mjs";

test("a sign-in with its terminal folded away still answers Windows' cursor query", async () => {
  const sockets = [];
  globalThis.WebSocket = class extends EventTarget {
    static OPEN = 1;
    readyState = 1;
    sent = [];
    constructor(url) { super(); this.url = String(url); sockets.push(this); }
    send(data) { this.sent.push(data); }
    close() { this.readyState = 3; }
  };
  const harness = { id: "codex", name: "Codex", installed: true, installBroken: false, authState: "needsLogin", needsConfigRepair: false };
  const { container } = await render(createElement(HarnessSetup, {
    harness,
    commands: { install: "", installUrl: null, managedFrom: "releases.openai.com", login: "codex login", update: "codex update", requiresNpm: false },
    action: "login",
  }));
  const [socket] = sockets;
  assert.equal(new URL(socket.url).searchParams.get("action"), "login");
  assert.equal(container.querySelector("button[aria-expanded]").getAttribute("aria-expanded"), "false");

  await act(() => {
    socket.onopen();
    // ConPTY's first and only output until a terminal answers.
    socket.onmessage({ data: new TextEncoder().encode("\x1b[6n").buffer });
  });
  await settle();
  const replies = socket.sent.filter((data) => typeof data !== "string").map((data) => new TextDecoder().decode(data));
  assert.ok(replies.some((reply) => /^\x1b\[\d+;\d+R$/.test(reply)), `no cursor report in ${JSON.stringify(replies)}`);

  const url = "https://auth.openai.com/oauth/authorize?client_id=app";
  await act(() => socket.onmessage({ data: JSON.stringify({ type: "signin", url }) }));
  await settle();
  assert.ok(container.querySelector(`a[href="${url}"]`), "the sign-in page is offered");
});

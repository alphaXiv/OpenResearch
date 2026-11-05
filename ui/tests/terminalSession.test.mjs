import assert from "node:assert/strict";
import test from "node:test";
import { Activity, act, createElement, useRef } from "react";
import { useTerminalSession } from "../src/components/useTerminalSession.ts";
import { render } from "./render.mjs";

/** A terminal pane in a workspace that Activity can hide; each connection it opens is numbered. */
async function pane(identity) {
  let opened = 0, closed = 0, session;
  function Terminal({ identity }) {
    const element = useRef(null);
    session = useTerminalSession(element, identity, () => ({ terminal: {}, dispose: () => closed++, number: ++opened }));
    return createElement("div", { ref: element });
  }
  const page = (mode, identity) => createElement(Activity, { mode }, createElement(Terminal, { identity }));
  const view = await render(page("visible", identity));
  const flush = () => act(async () => { await Promise.resolve(); });
  return {
    show: async (mode, next = identity) => { await view.rerender(page(mode, next)); await flush(); },
    unmount: async () => { await view.unmount(); await flush(); },
    get connection() { return session.current?.mounted.number; },
    get opened() { return opened; },
    get closed() { return closed; },
  };
}

test("hiding and reactivating a workspace preserves its terminal connection", async () => {
  const terminal = await pane("shell-a");
  await terminal.show("hidden");
  assert.equal(terminal.closed, 0);
  await terminal.show("visible");
  assert.equal(terminal.connection, 1);
  assert.equal(terminal.opened, 1);
  await terminal.unmount();
  assert.equal(terminal.closed, 1);
});

test("changing the terminal identity replaces only that connection", async () => {
  const terminal = await pane("shell-a");
  await terminal.show("visible", "shell-b");
  assert.equal(terminal.connection, 2);
  assert.equal(terminal.closed, 1);
  await terminal.unmount();
  assert.equal(terminal.closed, 2);
});

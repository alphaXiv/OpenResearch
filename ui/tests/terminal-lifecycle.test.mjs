import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import test from "node:test";

/** xterm 5.5's open() schedules a viewport initialization that fails once the terminal is disposed. */
globalThis.XtermWithInitTimer = class {
  static events = [];
  options = {};
  disposed = false;
  loadAddon() {}
  open(parent) {
    this.element = parent.appendChild(document.createElement("div"));
    setTimeout(() => XtermWithInitTimer.events.push(this.disposed ? "initialized after dispose" : "initialized"));
  }
  resize() {}
  onWriteParsed() { return { dispose() {} }; }
  onScroll() { return { dispose() {} }; }
  dispose() {
    this.disposed = true;
    XtermWithInitTimer.events.push("disposed");
  }
};
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier !== "@xterm/xterm") return nextResolve(specifier, context);
    return { url: "data:text/javascript,export const Terminal = globalThis.XtermWithInitTimer;", shortCircuit: true };
  },
});
const { mountTerminal } = await import("../src/components/terminal.ts");

test("immediate unmount lets xterm initialize before disposal and removes only its own element", async () => {
  const tick = () => new Promise((resolve) => setTimeout(resolve, 10));
  const wrap = document.body.appendChild(document.createElement("div"));
  const first = mountTerminal(wrap, false);
  first.dispose();
  const second = mountTerminal(wrap, false);
  assert.deepEqual([...wrap.children], [second.terminal.element]);
  await tick();
  assert.deepEqual(XtermWithInitTimer.events, ["initialized", "disposed", "initialized"]);

  second.dispose();
  await tick();
  assert.equal(wrap.children.length, 0);
  assert.equal(second.terminal.disposed, true);
});

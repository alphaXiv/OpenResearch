import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

test("immediate unmount lets xterm initialize before disposal and removes only its own element", () => {
  const timers = [];
  const events = [];
  const children = new Set();
  const observers = [];
  const wrap = {
    style: { overflowY: "" },
    addEventListener() {}, removeEventListener() {},
  };
  class Terminal {
    constructor() {
      this.element = { remove: () => children.delete(this.element) };
      this.options = {};
      this.disposed = false;
    }
    loadAddon() {}
    open() {
      children.add(this.element);
      timers.push(() => {
        assert.equal(this.disposed, false, "xterm's initial viewport callback needs its renderer");
        events.push("initialized");
      });
    }
    onWriteParsed() { return { dispose() {} }; }
    onScroll() { return { dispose() {} }; }
    dispose() { this.disposed = true; events.push("disposed"); }
  }
  class ResizeObserver {
    constructor() { observers.push(this); }
    observe() {}
    disconnect() { this.disconnected = true; }
  }
  const mocks = {
    "@xterm/xterm": { Terminal },
    "@xterm/addon-fit": { FitAddon: class { proposeDimensions() { return undefined; } } },
    "@xterm/addon-web-links": {},
  };
  const source = readFileSync(new URL("../src/components/terminal.ts", import.meta.url), "utf8");
  const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText;
  const exports = {};
  new Function("require", "exports", "document", "getComputedStyle", "ResizeObserver", "setTimeout", code)(
    (name) => mocks[name], exports, { documentElement: {} }, () => ({ getPropertyValue: () => "" }),
    ResizeObserver, (callback) => timers.push(callback),
  );

  const first = exports.mountTerminal(wrap, false);
  first.dispose();
  const second = exports.mountTerminal(wrap, false);
  assert.deepEqual([...children], [second.terminal.element]);
  assert.equal(observers[0].disconnected, true);
  while (timers.length) timers.shift()();
  assert.deepEqual(events, ["initialized", "disposed", "initialized"]);
  assert.equal(second.terminal.disposed, false);
  second.dispose();
  while (timers.length) timers.shift()();
  assert.equal(children.size, 0);
  assert.equal(second.terminal.disposed, true);
});

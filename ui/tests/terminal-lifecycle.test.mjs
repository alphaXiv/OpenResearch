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

test("OSC 8 and plain terminal links open HTTP URLs without a confirmation", () => {
  let options, webLinks;
  const opened = [];
  let title;
  let lines = [];
  class Terminal {
    constructor(value) {
      options = value;
      this.options = value;
      this.cols = 12;
      this.buffer = { active: { getLine: (row) => ({ translateToString: (_trim, start, end) => lines[row].slice(start, end) }) } };
      this.element = { remove() {}, setAttribute: (_name, value) => { title = value; }, removeAttribute: () => { title = undefined; } };
    }
    loadAddon(addon) { if (addon instanceof WebLinksAddon) webLinks = addon; }
    open() {}
    onWriteParsed() { return { dispose() {} }; }
    onScroll() { return { dispose() {} }; }
    dispose() {}
  }
  class WebLinksAddon { constructor(handler) { this.handler = handler; } }
  const mocks = {
    "@xterm/xterm": { Terminal },
    "@xterm/addon-fit": { FitAddon: class { proposeDimensions() { return undefined; } } },
    "@xterm/addon-web-links": { WebLinksAddon },
  };
  const source = readFileSync(new URL("../src/components/terminal.ts", import.meta.url), "utf8");
  const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText;
  const exports = {};
  new Function("require", "exports", "document", "window", "getComputedStyle", "ResizeObserver", "setTimeout", code)(
    (name) => mocks[name], exports, { documentElement: {} }, { open: (...args) => opened.push(args) },
    () => ({ getPropertyValue: () => "" }), class { observe() {} disconnect() {} }, () => {},
  );
  const wrap = { style: {}, addEventListener() {}, removeEventListener() {} };
  exports.mountTerminal(wrap, false, true);
  options.linkHandler.activate({}, "https://example.com/sign-in");
  webLinks.handler({}, "http://localhost:3000/callback");
  options.linkHandler.activate({}, "javascript:alert(1)");
  assert.deepEqual(opened.map(([url]) => url.href), ["https://example.com/sign-in", "http://localhost:3000/callback"]);
  assert.ok(opened.every(([, target, features]) => target === "_blank" && features === "noopener,noreferrer"));
  lines = ["https://exam", "ple.com/path"];
  const range = { start: { x: 1, y: 1 }, end: { x: 12, y: 2 } };
  options.linkHandler.hover({}, "https://example.com/path", range);
  assert.equal(title, undefined, "a visible URL needs no tooltip, even when wrapped");
  options.linkHandler.hover({}, "https://elsewhere.example", range);
  assert.equal(title, "https://elsewhere.example");
  options.linkHandler.leave();
  assert.equal(title, undefined);
});

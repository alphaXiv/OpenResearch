import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

function harness() {
  const slots = [];
  let cursor, effect, dispose;
  const exports = {};
  const code = ts.transpileModule(readFileSync(new URL("../src/components/useTerminalSession.ts", import.meta.url), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  new Function("require", "exports", code)(() => ({
    useRef: (value) => { const index = cursor++; return slots[index] ??= { current: value }; },
    useEffectEvent: (callback) => callback,
    useEffect: (callback) => { effect = callback; },
  }), exports);
  const element = { current: { isConnected: true } };
  let opened = 0, closed = 0;
  return {
    element,
    mount(id) {
      cursor = 0;
      const resource = exports.useTerminalSession(element, id, () => ({ terminal: {}, dispose: () => closed++, number: ++opened }));
      dispose = effect();
      return resource;
    },
    cleanup: () => dispose(),
    get opened() { return opened; }, get closed() { return closed; },
  };
}

const settle = () => new Promise(setImmediate);

test("hiding and reactivating a workspace preserves its terminal connection", async () => {
  const view = harness();
  const first = view.mount("shell-a").current;
  view.cleanup();
  await settle();
  assert.equal(view.closed, 0);
  assert.equal(view.mount("shell-a").current, first);
  assert.equal(view.opened, 1);
  view.element.current.isConnected = false;
  view.cleanup();
  await settle();
  assert.equal(view.closed, 1);
});

test("changing the terminal identity replaces only that connection", async () => {
  const view = harness();
  const first = view.mount("shell-a").current;
  view.cleanup();
  const second = view.mount("shell-b").current;
  await settle();
  assert.notEqual(first, second);
  assert.equal(view.opened, 2);
  assert.equal(view.closed, 1);
  view.element.current.isConnected = false;
  view.cleanup();
  await settle();
  assert.equal(view.closed, 2);
});

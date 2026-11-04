import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = ts.createSourceFile("ChatSearch.tsx", readFileSync(new URL("../src/components/ChatSearch.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let effect;
function visit(node) {
  if (ts.isCallExpression(node) && node.expression.getText(source) === "useEffect" && node.getText(source).includes('document.addEventListener("keydown"')) effect = node.arguments[0];
  ts.forEachChild(node, visit);
}
visit(source);
assert.ok(effect);

function shortcuts({ embedded = false, expanded = false, region = "left", sideSearch = false, inert = false } = {}) {
  const calls = [];
  let listener;
  const document = {
    querySelector: () => sideSearch ? {} : null,
    addEventListener(name, callback, capture) { assert.equal(name, "keydown"); assert.equal(capture, true); listener = callback; },
    removeEventListener(name, callback, capture) { assert.equal(name, "keydown"); assert.equal(capture, true); assert.equal(callback, listener); listener = null; },
  };
  const bindings = {
    document, embedded, expanded, selected: 2,
    field: { current: {
      closest: () => inert ? {} : null,
      focus: () => calls.push(["focus"]),
      select: () => calls.push(["select"]),
    } },
    setExpanded: value => calls.push(["expanded", value]),
    commandRegion: () => region,
    open: index => calls.push(["open", index]),
    move: (direction, navigate) => calls.push(["move", direction, navigate]),
  };
  const code = ts.transpileModule(effect.getText(source), { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  const close = new Function(...Object.keys(bindings), `return ${code}`)(...Object.values(bindings))();
  return {
    calls, close,
    press(key, modifiers = {}) {
      const event = {
        key, defaultPrevented: false, isComposing: false, altKey: false, ctrlKey: false, metaKey: false, shiftKey: false,
        stopped: false, ...modifiers,
        preventDefault() { this.defaultPrevented = true; },
        stopPropagation() { this.stopped = true; },
      };
      listener(event);
      return event;
    },
  };
}

test("find shortcuts keep navigating after focus moves into the transcript", () => {
  const find = shortcuts();
  for (const [key, modifiers] of [["F3", {}], ["F3", { shiftKey: true }], ["g", { metaKey: true }], ["g", { ctrlKey: true, shiftKey: true }]]) {
    const event = find.press(key, modifiers);
    assert.equal(event.defaultPrevented, true);
    assert.equal(event.stopped, true, "the same key must not cancel its own transcript jump");
  }
  assert.deepEqual(find.calls, [["move", 1, true], ["move", -1, true], ["move", 1, true], ["move", -1, true]]);
  find.close();
});

test("the main find box yields to side-chat search and handles preview panes", () => {
  const main = shortcuts({ region: "right", sideSearch: true });
  assert.equal(main.press("F3").defaultPrevented, false);
  assert.deepEqual(main.calls, []);
  const side = shortcuts({ embedded: true, region: "right" });
  side.press("F3");
  assert.deepEqual(side.calls, [["move", 1, true]]);
  const preview = shortcuts({ region: "right" });
  preview.press("F3");
  assert.deepEqual(preview.calls, [["move", 1, true]]);
  for (const find of [main, side, preview]) find.close();
});

test("typing, composition, other modifiers, and inert find boxes retain their keys", () => {
  const find = shortcuts();
  for (const [key, modifiers] of [["g", {}], ["f", {}], ["f", { metaKey: true, ctrlKey: true }], ["F3", { isComposing: true }], ["F3", { altKey: true }], ["F3", { ctrlKey: true }]]) assert.equal(find.press(key, modifiers).defaultPrevented, false);
  assert.deepEqual(find.calls, []);
  const inert = shortcuts({ inert: true });
  assert.equal(inert.press("F3").defaultPrevented, false);
  assert.deepEqual(inert.calls, []);
  find.close();
  inert.close();
});

test("the expanded result picker opens its selected result before iterating", () => {
  const find = shortcuts({ expanded: true });
  find.press("F3");
  find.press("F3", { shiftKey: true });
  assert.deepEqual(find.calls, [["open", 2], ["move", -1, true]]);
  find.close();
});

test("Mod+F refocuses the existing find box while reading a preview", () => {
  const find = shortcuts({ region: "right" });
  const event = find.press("f", { metaKey: true });
  assert.equal(event.defaultPrevented, true);
  assert.equal(event.stopped, true);
  assert.deepEqual(find.calls, [["expanded", true], ["focus"], ["select"]]);
  find.close();
});

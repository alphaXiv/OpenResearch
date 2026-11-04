import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import { defaultRangeExtractor } from "@tanstack/react-virtual";

const file = ts.createSourceFile("ChatPanel.tsx", readFileSync(new URL("../src/components/ChatPanel.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
function find(predicate) {
  let found;
  function visit(node) {
    if (predicate(node)) found = node;
    ts.forEachChild(node, visit);
  }
  visit(file);
  assert.ok(found);
  return found;
}
function evaluate(code, bindings) {
  const js = ts.transpileModule(code, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  return new Function(...Object.keys(bindings), js)(...Object.values(bindings));
}

test("virtual history retains selections and dirty forms, then releases closed forms", () => {
  const expression = find(node => ts.isVariableDeclaration(node) && node.name.getText(file) === "rangeExtractor").initializer;
  const draftRows = { current: new Set(["a", "deleted"]) };
  const selectedRows = new Set(["b"]);
  let formOpen = true;
  const extract = evaluate(`return ${expression.getText(file)}`, {
    useCallback: callback => callback,
    rowIndexes: new Map(["a", "b", "c", "d", "e"].map((id, index) => [id, index])),
    draftRows, selectedRows, focusedRow: null, jumping: false, targetIndex: undefined,
    scrollRef: { current: { querySelector: () => ({ querySelector: () => formOpen }) } },
    CSS: { escape: value => value }, defaultRangeExtractor,
  });
  const range = { startIndex: 3, endIndex: 4, overscan: 0, count: 5 };
  assert.deepEqual(extract(range), [0, 1, 3, 4]);
  formOpen = false;
  selectedRows.clear();
  assert.deepEqual(extract(range), [3, 4]);
  assert.equal(draftRows.current.size, 0);
});

test("tool labels reuse immutable parts but refresh after stream replacement or locale change", () => {
  const declaration = find(node => ts.isFunctionDeclaration(node) && node.name?.text === "toolActivity");
  let locale = "en", calls = 0;
  const activity = evaluate(`const toolActivities = new WeakMap(); ${declaration.getText(file)}; return toolActivity;`, {
    getLocale: () => locale,
    computeToolActivity: part => { calls++; return { label: `${locale}:${part.state.status}` }; },
  });
  const original = { id: "tool-1", state: { status: "running" } };
  assert.strictEqual(activity(original), activity(original));
  assert.equal(calls, 1);
  assert.equal(activity({ ...original, state: { status: "completed" } }).label, "en:completed");
  locale = "es";
  assert.equal(activity(original).label, "es:running");
  assert.equal(calls, 3);
});

test("the focused article stays mounted outside the viewport without being retained", () => {
  const expression = find(node => ts.isVariableDeclaration(node) && node.name.getText(file) === "rangeExtractor").initializer;
  const draftRows = { current: new Set() };
  const extract = focusedRow => evaluate(`return ${expression.getText(file)}`, {
    useCallback: callback => callback,
    rowIndexes: new Map(["a", "b", "c"].map((id, index) => [id, index])),
    draftRows, selectedRows: new Set(), focusedRow, jumping: false, targetIndex: undefined, scrollRef: { current: null }, defaultRangeExtractor,
  });
  const range = { startIndex: 2, endIndex: 2, overscan: 0, count: 3 };
  assert.deepEqual(extract("a")(range), [0, 2]);
  assert.deepEqual(extract(null)(range), [2]);
  assert.equal(draftRows.current.size, 0);
});

test("focus and dirty form input retain rows; ordinary clicks retain nothing", () => {
  const handler = name => find(node => ts.isJsxAttribute(node) && node.name.getText(file) === name && node.parent.getText(file).includes('role="article"')).initializer.expression;
  const draftRows = { current: new Set() };
  let focusedRow = null;
  const bindings = {
    draftRows, rowId: "a",
    setFocusedRow: id => { focusedRow = id; },
    stickToBottom: { current: true },
    virtualizer: { options: {}, setOptions() {} },
    Element: class {},
  };
  const run = (name, event = {}) => evaluate(`return ${handler(name).getText(file)}`, bindings)(event);
  run("onFocusCapture");
  assert.equal(focusedRow, "a");
  assert.equal(draftRows.current.size, 0, "a screen reader moving focus while reading retains nothing");
  run("onClickCapture", { target: {} });
  assert.equal(draftRows.current.size, 0);
  run("onInputCapture");
  assert.deepEqual([...draftRows.current], ["a"]);
});

test("feed keys move focus between articles and leave the feed", () => {
  const handler = find(node => ts.isJsxAttribute(node) && node.name.getText(file) === "onKeyDown" && node.getText(file).includes("PageDown")).initializer.expression;
  const focused = [], left = [];
  class HTMLElement {
    constructor(index, editable) { Object.assign(this, { index, editable, isContentEditable: false }); }
    matches() { return this.editable; }
    closest() { return { dataset: { index: String(this.index) } }; }
  }
  const keydown = evaluate(`return ${handler.getText(file)}`, {
    HTMLElement,
    visibleMessages: ["a", "b", "c"].map(id => [{ id }]),
    setFocusedRow: id => focused.push(id),
    onLeaveFeed: forward => left.push(forward),
  });
  const press = (key, { index = 1, ctrlKey = false, shiftKey = false, editable = false } = {}) => {
    let prevented = false;
    keydown({
      key, ctrlKey, shiftKey, altKey: false, metaKey: false,
      target: new HTMLElement(index, editable),
      preventDefault: () => { prevented = true; },
    });
    return prevented;
  };
  assert.equal(press("PageDown"), true);
  assert.equal(press("PageUp"), true);
  assert.deepEqual(focused, ["c", "a"]);
  assert.equal(press("PageDown", { index: 2 }), false, "the last article lets the page scroll");
  assert.equal(press("PageDown", { editable: true }), false, "text fields keep their own keys");
  assert.equal(press("PageDown", { ctrlKey: true }), false, "Ctrl+PageDown stays the browser's");
  assert.equal(press("PageDown", { shiftKey: true }), false);
  assert.equal(press("End", { ctrlKey: true }), true);
  assert.equal(press("Home", { ctrlKey: true }), true);
  assert.deepEqual(left, [true, false]);
  assert.deepEqual(focused, ["c", "a"]);
});

test("viewport and footer resizing preserve bottom pin without moving a reader", () => {
  const effect = find(node => ts.isCallExpression(node) && node.expression.getText(file) === "useEffect" && node.getText(file).includes("observer.observe(inner)")).arguments[0];
  let resize, pinned = 0, updated = 0, disconnected = false;
  const observed = [], el = { scrollHeight: 1000, scrollTop: 500, clientHeight: 500 }, inner = {}, stickToBottom = { current: true };
  const setup = evaluate(`return ${effect.getText(file)}`, {
    threadRef: { current: el }, threadInnerRef: { current: inner }, stickToBottom,
    scrollToEndRef: { current: () => { pinned++; } },
    setTranscriptAtBottom: value => { assert.equal(value, el.scrollHeight - el.scrollTop - el.clientHeight < 60); updated++; },
    ResizeObserver: class {
      constructor(callback) { resize = callback; }
      observe(target) { observed.push(target); }
      disconnect() { disconnected = true; }
    },
  });
  const cleanup = setup();
  assert.deepEqual(observed, [el, inner]);
  resize();
  assert.equal(pinned, 1);
  stickToBottom.current = false;
  resize();
  assert.equal(pinned, 1);
  assert.equal(updated, 1);
  assert.equal(stickToBottom.current, false);
  el.scrollHeight += 400;
  resize();
  assert.equal(pinned, 1);
  assert.equal(stickToBottom.current, false);
  cleanup();
  assert.equal(disconnected, true);
});

test("mounting a different conversation resets both bottom-pin representations", () => {
  const expression = find(node => ts.isVariableDeclaration(node) && node.name.getText(file) === "pinTranscriptToBottom").initializer;
  const effect = find(node => ts.isCallExpression(node) && node.expression.getText(file) === "useLayoutEffect" && node.getText(file).includes("onPinToBottom()")).arguments[0];
  const stickToBottom = { current: false }, scrollingByUser = { current: true }, scrollToEndRef = { current: null };
  let atBottom = false, scrolls = 0;
  const pin = evaluate(`return ${expression.getText(file)}`, {
    useCallback: callback => callback, stickToBottom, scrollingByUser, scrollToEndRef,
    setTranscriptAtBottom: value => { atBottom = value; },
  });
  const mount = evaluate(`return ${effect.getText(file)}`, {
    virtualizer: { scrollToOffset: offset => { assert.equal(offset, 1000); scrolls++; } }, scrollRef: { current: { scrollHeight: 1000 } }, scrollToEndRef, onPinToBottom: pin,
  });
  const cleanup = mount();
  assert.equal(stickToBottom.current, true);
  assert.equal(atBottom, true);
  assert.equal(scrolls, 1);
  cleanup();
  assert.equal(scrollToEndRef.current, null);
});

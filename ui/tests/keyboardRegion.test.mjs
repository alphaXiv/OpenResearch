import { readFileSync } from "node:fs";
import ts from "typescript";
import assert from "node:assert/strict";
import { test } from "node:test";

let pointerdown;
const focusListeners = new Map();
class Element {
  constructor(...ancestors) { this.ancestors = ancestors; }
  closest(selectors) { return selectors.split(", ").some((selector) => this.ancestors.includes(selector)) ? this : null; }
}
globalThis.Element = Element;
globalThis.window = {
  addEventListener(type, listener) { if (type === "pointerdown") pointerdown = listener; else focusListeners.set(type, listener); },
  removeEventListener(type) { focusListeners.delete(type); },
};
globalThis.document = { body: new Element(), activeElement: null };
const { commandRegion, subscribeCommandRegion } = await import("../src/keyboardRegion.ts");

test("focus chooses the left session or right tab, including embedded side chats", () => {
  assert.equal(commandRegion(new Element(".session-rail")), "left");
  assert.equal(commandRegion(new Element(".chat-pane")), "left");
  assert.equal(commandRegion(new Element(".chat-pane", ".right-pane")), "right");
  assert.equal(commandRegion(new Element()), null);
});

test("body focus falls back to the last clicked pane, including native menu commands", () => {
  pointerdown({ target: new Element(".right-pane") });
  document.activeElement = document.body;
  assert.equal(commandRegion(), "right");
  pointerdown({ target: new Element(".session-rail") });
  assert.equal(commandRegion(), "left");
  assert.equal(commandRegion(new Element(".right-pane")), "right");
  pointerdown({ target: new Element() });
  assert.equal(commandRegion(), null);
});

test("the Windows menu preserves its originating pane", () => {
  pointerdown({ target: new Element(".right-pane") });
  pointerdown({ target: new Element("[role=menubar]") });
  assert.equal(commandRegion(new Element("[role=menu]")), "right");
});

const source = ts.createSourceFile("App.tsx", readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let closeCallback;
function visit(node) {
  if (ts.isCallExpression(node) && node.expression.getText(source) === "useHotkey" && node.arguments[0]?.getText(source) === 'appHotkeys["tab.close"]') closeCallback = node.arguments[1].getText(source);
  ts.forEachChild(node, visit);
}
visit(source);
assert.ok(closeCallback);

for (const [tab, action] of [
  ["files", "home"], ["terminal", "home"], ["artifacts", "home"], ["experiments", "home"],
  [{ code: "e" }, "code"], [{ path: "paper.md" }, "file"], [{ experimentId: "e" }, "experiment"],
  [{ kind: "plan" }, "plan"], [{ kind: "subagent" }, "subagent"], [{ kind: "side" }, "side"],
]) {
  test(`Close Tab routes the focused right tab through its ${action} close path`, () => {
    document.activeElement = new Element(".right-pane");
    const called = [];
    const bindings = { commandRegion, mainView: "chat", activeSessionId: "s", panelOpen: true, rightTab: tab };
    for (const [name, label] of Object.entries({ closeHomeTab: "home", closeCodeTab: "code", closeFileTab: "file", closeExperimentTab: "experiment", closePlanTab: "plan", closeSubagentTab: "subagent", closeSideTab: "side", selectMainView: "view", onActiveSessionChange: "session" })) bindings[name] = (...args) => called.push([label, ...args]);
    new Function(...Object.keys(bindings), `return (${closeCallback})`)(...Object.values(bindings))();
    assert.deepEqual(called, [[action, tab]]);
  });
}

test("Close Tab on the left deselects sessions or leaves settings, without closing right tabs", () => {
  document.activeElement = new Element(".session-rail");
  for (const [mainView, activeSessionId, expected] of [["chat", "s", ["session", null]], ["settings", "s", ["view", "chat"]], ["chat", null, null]]) {
    const called = [];
    const bindings = { commandRegion, mainView, activeSessionId, panelOpen: true, rightTab: "files", selectMainView: (view) => called.push(["view", view]), onActiveSessionChange: (id) => called.push(["session", id]), closeHomeTab: () => assert.fail("left focus must not close a right tab") };
    new Function(...Object.keys(bindings), `return (${closeCallback})`)(...Object.values(bindings))();
    assert.deepEqual(called, expected ? [expected] : []);
  }
});

test("Close Tab does nothing outside a pane or while the right pane is closed", () => {
  for (const target of [new Element(), new Element(".right-pane")]) {
    document.activeElement = target;
    const bindings = { commandRegion, mainView: "chat", panelOpen: false, closeHomeTab: () => assert.fail("nothing is closable") };
    new Function(...Object.keys(bindings), `return (${closeCallback})`)(...Object.values(bindings))();
  }
});

test("keyboard entry into the Windows menu preserves the previously focused pane", async () => {
  pointerdown({ target: new Element() });
  const release = subscribeCommandRegion(() => {});
  const pane = new Element(".right-pane");
  const menu = new Element("[role=menubar]");
  document.activeElement = menu;
  focusListeners.get("focusin")({ type: "focusin", target: menu, relatedTarget: pane });
  await Promise.resolve();
  assert.equal(commandRegion(), "right");
  release();
});

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const file = ts.createSourceFile("App.tsx", readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let resizeEffect;
let focusEffect;
function visit(node) {
  if (ts.isCallExpression(node) && node.expression.getText(file) === "useEffect" && node.getText(file).includes("const onResize =")) resizeEffect = node;
  if (ts.isCallExpression(node) && node.expression.getText(file) === "useLayoutEffect" && node.arguments[1]?.getText(file) === "[railVisible]") focusEffect = node;
  ts.forEachChild(node, visit);
}
visit(file);
assert.ok(resizeEffect && focusEffect);

function scenario({ focusedInChat = true, railOpen = true } = {}) {
  const body = { closest: () => null };
  const button = {
    isConnected: true,
    closest: (selector) => focusedInChat && selector === ".chat-pane" ? {} : null,
    getAttribute: () => "Show sidebar",
    getClientRects: () => [{}],
    focus() { document.activeElement = button; },
  };
  const document = { body, activeElement: button };
  const hiddenRailButton = { getClientRects: () => [] };
  const railButton = { getClientRects: () => [{}], focus() { document.activeElement = railButton; } };
  const focusAfterRailChange = { current: null };
  const lastFocused = { current: button };
  const bodyRef = { current: {
    querySelectorAll: () => [hiddenRailButton, railButton],
    parentElement: { querySelectorAll: () => [button] },
  } };
  const getComputedStyle = () => ({ visibility: "visible" });
  const m = { chat_panel_show_sidebar: () => "Show sidebar" };
  let onResize;
  const window = { innerWidth: 1040, addEventListener: (_, callback) => { onResize = callback; }, removeEventListener() {} };
  const resize = ts.transpile(resizeEffect.getText(file));
  new Function("useEffect", "window", "document", "railOpen", "railTemporarilyHidden", "railWidth", "sidebarFitsWithPanel", "focusAfterRailChange", "setWindowWidth", "m", resize)(
    (effect) => effect(), window, document, railOpen, true, 300, (width, railWidth) => width >= railWidth + 740, focusAfterRailChange, () => {}, m,
  );
  const focus = ts.transpile(focusEffect.getText(file));
  const restore = (railVisible = true) => new Function("useLayoutEffect", "railVisible", "lastFocused", "document", "focusAfterRailChange", "bodyRef", "getComputedStyle", "m", focus)(
    (effect) => effect(), railVisible, lastFocused, document, focusAfterRailChange, bodyRef, getComputedStyle, m,
  );
  return { onResize, restore, button, body, document, railButton, focusAfterRailChange };
}

test("resize restores focus when the focused show-sidebar button disappears", () => {
  const s = scenario();
  s.onResize();
  s.button.isConnected = false;
  s.document.activeElement = s.body;
  s.restore();
  assert.equal(s.document.activeElement, s.railButton);
});

test("closing the sidebar focuses the show button outside the app body", () => {
  const s = scenario();
  s.focusAfterRailChange.current = "show";
  s.document.activeElement = s.body;
  s.restore(false);
  assert.equal(s.document.activeElement, s.button);
});

test("resize leaves focus elsewhere and respects a collapsed sidebar", () => {
  for (const options of [{ focusedInChat: false }, { railOpen: false }]) {
    const s = scenario(options);
    s.onResize();
    assert.equal(s.focusAfterRailChange.current, null);
  }
  const blurred = scenario();
  blurred.document.activeElement = blurred.body;
  blurred.onResize();
  assert.equal(blurred.focusAfterRailChange.current, null);
  const s = scenario();
  s.onResize();
  s.button.isConnected = false;
  const otherControl = {};
  s.document.activeElement = otherControl;
  s.restore();
  assert.equal(s.document.activeElement, otherControl);
});

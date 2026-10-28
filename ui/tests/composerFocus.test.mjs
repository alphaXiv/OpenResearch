import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = ts.createSourceFile("ChatPanel.tsx", readFileSync(new URL("../src/components/ChatPanel.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let focusEffect;
function visit(node) {
  if (ts.isCallExpression(node) && node.expression.getText(source) === "useEffect"
    && node.arguments[1]?.getText(source).includes("composerFocusNonce")) focusEffect = node;
  ts.forEachChild(node, visit);
}
visit(source);
assert.ok(focusEffect);

test("the embedded side-chat caller uses activation requests without remounting", () => {
  const app = ts.createSourceFile("App.tsx", readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const callers = [];
  function visit(node) {
    if (ts.isJsxSelfClosingElement(node) && node.tagName.getText(app) === "ChatPanel"
      && node.attributes.properties.some(prop => ts.isJsxAttribute(prop)
        && prop.name.text === "embedded")) callers.push(node);
    ts.forEachChild(node, visit);
  }
  visit(app);
  assert.equal(callers.length, 1);
  const attributes = callers[0].attributes.properties.filter(ts.isJsxAttribute);
  assert.ok(attributes.some(prop => prop.name.text === "embedded"));
  const nonce = attributes.find(prop => prop.name.text === "composerFocusNonce")?.initializer;
  assert.equal(nonce?.getText(app), "{sideComposerFocusNonce}");
  assert.ok(!attributes.some(prop => prop.name.text === "key"));
  const promotion = attributes.find(prop => prop.name.text === "onActiveSessionChange").initializer.getText(app);
  assert.match(promotion, /openSideTab\(sessionId\)/);
  assert.doesNotMatch(promotion, /focusSideTab|setSideComposerFocusNonce/);
  assert.match(app.text, /onSelectSideChat=\{focusSideTab\}/);
});

test("repeated explicit openings request focus while navigation alone does not", () => {
  const app = ts.createSourceFile("App.tsx", readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let activation;
  let start;
  function visit(node) {
    if (ts.isVariableDeclaration(node) && node.name.getText(app) === "focusSideTab") activation = node.initializer;
    if (ts.isVariableDeclaration(node) && node.name.getText(app) === "startSideChat") start = node.initializer;
    ts.forEachChild(node, visit);
  }
  visit(app);
  assert.ok(activation);
  assert.match(start.getText(app), /focusSideTab\(null\)/);
  assert.match(start.getText(app), /focusSideTab\(session.id\)/);
  let nonce = 0;
  const opened = [];
  const navigate = id => opened.push(id);
  const activate = new Function("useCallback", "setSideComposerFocusNonce", "openSideTab",
    `return ${ts.transpile(activation.getText(app))}`)(
    callback => callback,
    update => { nonce = update(nonce); },
    navigate,
  );
  for (const id of [null, null, "existing-side"]) activate(id);
  assert.equal(nonce, 3);
  navigate("persisted-draft");
  assert.equal(nonce, 3);
  assert.deepEqual(opened, [null, null, "existing-side", "persisted-draft"]);
});

function composer() {
  const frames = new Map();
  const document = { activeElement: null };
  const calls = [];
  const focusedComposerNonce = { current: 0 };
  let nextFrame = 0;
  let cleanup;
  let previousDeps;
  const input = {
    value: "side question",
    focus() { document.activeElement = input; calls.push("focus"); },
    setSelectionRange(start, end) { calls.push([start, end]); },
    scrollIntoView() {},
  };
  const bindings = {
    composerFocusNonce: 0, contentLoading: false, contentError: null, historyLoading: false,
    newChatFocusProject: null, projectId: "p", activeId: null, mainView: "chat",
    setNewChatFocusProject(value) { bindings.newChatFocusProject = value; },
    focusedComposerNonce, composerRef: { current: input }, document,
    window: {
      requestAnimationFrame(callback) { frames.set(++nextFrame, callback); return nextFrame; },
      cancelAnimationFrame(id) { frames.delete(id); },
    },
  };
  return {
    calls, document, input,
    render(props) {
      Object.assign(bindings, props);
      const evaluate = node => new Function(...Object.keys(bindings), `return (${node.getText(source)})`)(...Object.values(bindings));
      const deps = evaluate(focusEffect.arguments[1]);
      if (previousDeps?.every((value, index) => Object.is(value, deps[index]))) return;
      cleanup?.();
      previousDeps = deps;
      cleanup = evaluate(focusEffect.arguments[0])();
    },
    flush() {
      const pending = [...frames.values()];
      frames.clear();
      pending.forEach(callback => callback());
    },
  };
}

for (const loading of ["contentLoading", "contentError", "historyLoading"]) {
  test(`composer focus waits for ${loading} and does not steal focus afterward`, () => {
    const chat = composer();
    chat.render({ composerFocusNonce: 1, [loading]: true });
    chat.flush();
    assert.deepEqual(chat.calls, []);
    chat.render({ [loading]: false });
    chat.flush();
    assert.equal(chat.document.activeElement, chat.input);
    assert.deepEqual(chat.calls, ["focus", [13, 13]]);
    const main = {};
    chat.document.activeElement = main;
    chat.render({ [loading]: true });
    chat.render({ [loading]: false });
    chat.flush();
    assert.equal(chat.document.activeElement, main);
    assert.equal(chat.calls.length, 2);
  });
}

test("a new nonce requests focus, but the default and unchanged nonce do not", () => {
  const chat = composer();
  chat.render({});
  chat.flush();
  assert.deepEqual(chat.calls, []);
  for (const nonce of [1, 2]) {
    chat.render({ composerFocusNonce: nonce });
    chat.flush();
    assert.equal(chat.calls.length, nonce * 2);
    chat.render({});
    chat.flush();
    assert.equal(chat.calls.length, nonce * 2);
  }
});

test("loading cancels scheduled focus without consuming its request", () => {
  const chat = composer();
  chat.render({ composerFocusNonce: 1 });
  chat.render({ historyLoading: true });
  chat.flush();
  assert.deepEqual(chat.calls, []);
  chat.render({ historyLoading: false });
  chat.flush();
  assert.equal(chat.document.activeElement, chat.input);
});

test("unsuccessful focus leaves the request pending", () => {
  const chat = composer();
  const focus = chat.input.focus;
  chat.input.focus = () => {};
  chat.render({ composerFocusNonce: 1 });
  chat.flush();
  assert.deepEqual(chat.calls, []);
  chat.input.focus = focus;
  chat.render({ historyLoading: true });
  chat.render({ historyLoading: false });
  chat.flush();
  assert.equal(chat.document.activeElement, chat.input);
});

test("new-chat focus waits for the requested project and new-chat route", () => {
  const chat = composer();
  chat.render({ newChatFocusProject: "other", activeId: "existing" });
  chat.flush();
  assert.deepEqual(chat.calls, []);
  chat.render({ projectId: "other", activeId: null, mainView: "settings" });
  chat.flush();
  assert.deepEqual(chat.calls, []);
  chat.render({ mainView: "chat" });
  chat.flush();
  assert.equal(chat.document.activeElement, chat.input);
});

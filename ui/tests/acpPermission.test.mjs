import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";

const source = ts.createSourceFile("ChatPanel.tsx", readFileSync(new URL("../src/components/ChatPanel.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const declaration = source.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === "PromptCard");
assert.ok(declaration);
const compiled = ts.transpileModule(declaration.getText(source), { compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX } }).outputText;
const makeCard = (useState = React.useState) => new Function("require", "useState", "m", "Button", "TriangleAlert", "inputString", "permissionActivityLabel", "Textarea", "PROMPT_HEAD_CLASS_NAME", "PROMPT_COLLAPSED_CLASS_NAME", "PROMPT_COLLAPSED_BODY_CLASS_NAME", "AnnotationsPopover", "HelpCircle", "IconButton", "X", "Pencil", "Check", "ChevronRight", `const exports = {}; ${compiled}; return PromptCard;`)(
  createRequire(import.meta.url), useState,
  new Proxy({}, { get: (_, name) => () => String(name) }),
  ({ children, size, variant, ...props }) => React.createElement("button", props, children),
  () => null, () => "", () => "Native tool",
  (props) => React.createElement("textarea", props), "", "", "", () => null, () => null, ({ children, ...props }) => React.createElement("button", props, children), () => null, () => null, () => null, () => null,
);
const Card = makeCard();

test("native permission cards render exact agent labels in their original order", () => {
  const part = { id: "permission", prompt: { kind: "permission", header: "Inspect the file", nativeChoices: [
    { id: "opaque/reject", label: "Skip this operation", kind: "reject_once" },
    { id: "opaque/allow", label: "Allow exactly once", kind: "allow_once" },
  ] } };
  const html = renderToStaticMarkup(React.createElement(Card, { part, onRespond: () => {} }));
  assert.ok(html.includes("Inspect the file"));
  assert.ok(html.indexOf("Skip this operation") < html.indexOf("Allow exactly once"));
  assert.equal((html.match(/<button/g) ?? []).length, 2);
  assert.equal(renderToStaticMarkup(React.createElement(Card, { part: { ...part, prompt: { ...part.prompt, resolved: true } } })), "");
});

test("a free-text question provides its own labeled answer field and submit action", () => {
  const part = { id: "question", prompt: { kind: "question", question: "Can you free space?" } };
  const html = renderToStaticMarkup(React.createElement(Card, { part, onRespond: async () => true }));
  assert.match(html, /<form/);
  assert.match(html, /<textarea[^>]+aria-label="chat_question_your_answer"/);
  assert.match(html, /<button[^>]+type="submit"[^>]+disabled/);
  assert.match(html, /chat_panel_send/);
  const readonly = renderToStaticMarkup(React.createElement(Card, { part }));
  assert.doesNotMatch(readonly, /<form|<textarea/);
});

test("answered questions show the agent question followed by one quoted answer", () => {
  const part = { id: "question", prompt: { kind: "question", question: "Can you free space?", resolved: true, note: "What is using memory?" } };
  const html = renderToStaticMarkup(React.createElement(Card, { part, answerInTranscript: true }));
  assert.match(html, /Can you free space/);
  assert.doesNotMatch(html, /What is using memory/);
  const native = renderToStaticMarkup(React.createElement(Card, { part }));
  assert.match(native, /What is using memory/);
  assert.match(native, /truncate[^>]*>Can you free space/);
  assert.equal((native.match(/Can you free space/g) ?? []).length, 2);
});

test("custom answers submit explicitly and keep their draft when delivery fails", async () => {
  const state = [];
  let cursor = 0;
  const TestCard = makeCard((initial) => {
    const index = cursor++;
    if (!(index in state)) state[index] = initial;
    return [state[index], (value) => { state[index] = typeof value === "function" ? value(state[index]) : value; }];
  });
  let finish;
  const deliveries = [];
  const props = {
    part: { id: "question", prompt: { kind: "question", question: "Can you free space?" } },
    onRespond: (answer) => {
      deliveries.push(answer);
      return new Promise((resolve) => { finish = resolve; });
    },
  };
  const render = () => { cursor = 0; return TestCard(props); };
  const find = (element, predicate) => {
    if (!React.isValidElement(element)) return undefined;
    if (predicate(element)) return element;
    for (const child of React.Children.toArray(element.props.children)) {
      const match = find(child, predicate);
      if (match) return match;
    }
  };
  const field = (tree) => find(tree, (node) => node.props["aria-label"] === "chat_question_your_answer");
  field(render()).props.onChange({ target: { value: "What is using memory?\nPlease list the files." } });
  assert.equal(deliveries.length, 0);
  find(render(), (node) => node.type === "form").props.onSubmit({ preventDefault() {} });
  assert.deepEqual(deliveries, [{ promptId: "question", answers: [], note: "What is using memory?\nPlease list the files." }]);
  assert.equal(field(render()).props.disabled, true);
  finish(false);
  await new Promise(setImmediate);
  assert.equal(field(render()).props.value, "What is using memory?\nPlease list the files.");
  assert.equal(field(render()).props.disabled, false);
  find(render(), (node) => node.type === "form").props.onSubmit({ preventDefault() {} });
  finish(true);
  await new Promise(setImmediate);
  assert.equal(field(render()).props.value, "");
});

test("question choices wait for Send, custom text replaces a choice, and Skip is explicit", async () => {
  const state = [];
  let cursor = 0;
  const TestCard = makeCard((initial) => {
    const index = cursor++;
    if (!(index in state)) state[index] = initial;
    return [state[index], (value) => { state[index] = typeof value === "function" ? value(state[index]) : value; }];
  });
  const deliveries = [];
  const props = {
    part: { id: "question", prompt: { kind: "question", question: "Which one?", options: [{ label: "First" }, { label: "Second" }] } },
    onRespond: async (answer) => { deliveries.push(answer); return true; },
  };
  const render = () => { cursor = 0; return TestCard(props); };
  const find = (element, predicate) => {
    if (!React.isValidElement(element)) return undefined;
    if (predicate(element)) return element;
    for (const child of React.Children.toArray(element.props.children)) {
      const match = find(child, predicate);
      if (match) return match;
    }
  };
  find(render(), (node) => node.props.className?.includes("prompt-option ")).props.onClick();
  assert.equal(deliveries.length, 0);
  assert.equal(find(render(), (node) => node.props["aria-pressed"] === true).props.type, "button");
  find(render(), (node) => node.type === "form").props.onSubmit({ preventDefault() {} });
  await new Promise(setImmediate);
  assert.deepEqual(deliveries[0], { promptId: "question", answers: ["First"], note: undefined });
  find(render(), (node) => node.props["aria-label"] === "chat_question_your_answer").props.onChange({ target: { value: "Something else" } });
  assert.equal(find(render(), (node) => node.props["aria-pressed"] === true), undefined);
  find(render(), (node) => node.type === "form").props.onSubmit({ preventDefault() {} });
  await new Promise(setImmediate);
  assert.deepEqual(deliveries[1], { promptId: "question", answers: [], note: "Something else" });
  find(render(), (node) => node.props.children === "chat_question_skip").props.onClick();
  await new Promise(setImmediate);
  assert.deepEqual(deliveries[2], { promptId: "question", approve: false });
});

test("skipped questions show their outcome while preserving historical answer text", () => {
  const part = { id: "question", prompt: { kind: "question", question: "Which color?", resolved: true, approved: false } };
  const html = renderToStaticMarkup(React.createElement(Card, { part }));
  assert.match(html, /chat_question_skipped/);
  assert.match(html, /<details[^>]*><summary/);
  assert.doesNotMatch(html, /bg-panel/);
  const historical = renderToStaticMarkup(React.createElement(Card, { part: { ...part, prompt: { ...part.prompt, note: "Previously entered answer" } } }));
  assert.match(historical, /Previously entered answer/);
  assert.doesNotMatch(historical, /chat_question_skipped/);
  const stale = renderToStaticMarkup(React.createElement(Card, { part: { ...part, prompt: { ...part.prompt, approved: undefined } } }));
  assert.doesNotMatch(stale, /chat_question_skipped/);
});

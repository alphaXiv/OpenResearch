import { EditorView } from "@codemirror/view";
import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement, Fragment, useRef } from "react";
import { ComposerEditor } from "../src/components/ComposerEditor.tsx";
import { useComposerFocus } from "../src/components/useComposerFocus.ts";
import { m } from "../src/paraglide/messages.js";
import { renderChat, session } from "./chat.mjs";
import { find, render, settle } from "./render.mjs";

let frames = [];
const { requestAnimationFrame, cancelAnimationFrame } = window;
test.beforeEach(() => {
  frames = [];
  window.requestAnimationFrame = (callback) => frames.push(callback);
  window.cancelAnimationFrame = (id) => { frames[id - 1] = null; };
});
test.afterEach(() => Object.assign(window, { requestAnimationFrame, cancelAnimationFrame }));
const flush = () => act(async () => { for (const frame of frames.splice(0)) frame?.(0); });

const chips = { isCommand: () => false, skill: () => undefined, message: () => undefined, chat: () => undefined, isPath: () => false, attachment: () => undefined };
const ignore = () => {};

let requestNewChat;
function Composer({ nonce = 0, ready = true, projectId = "p", activeId = null, inert = false }) {
  const ref = useRef(null);
  requestNewChat = useComposerFocus(ref, nonce, ready, projectId, activeId);
  return createElement(Fragment, null,
    createElement("div", { inert },
      createElement(ComposerEditor, {
        ref, value: "side question", onChange: ignore, onKeyDown: ignore, onSubmit: ignore, onFiles: ignore, onPaste: ignore,
        placeholder: "Message", plain: false, chipSource: chips, projectId, draftKey: "draft", resizeKey: "composer-height",
      })),
    createElement("button", null, "elsewhere"),
    createElement("div", { role: "search" }, createElement("input", { "aria-label": "Find" })));
}

/** A composer whose props follow each `update`. */
async function composer(initial = {}) {
  let props = initial;
  const view = await render(createElement(Composer, props));
  const field = view.container.querySelector(".cm-content");
  return {
    field,
    focused: () => document.activeElement === field,
    caret: () => EditorView.findFromDOM(field).state.selection.main.head,
    update: (next) => view.rerender(createElement(Composer, props = { ...props, ...next })),
    elsewhere: () => act(async () => view.container.querySelector("button").focus()),
    search: () => act(async () => view.container.querySelector("[role=search] input").focus()),
  };
}

test("composer focus waits until the chat is ready and does not steal focus afterward", async () => {
  const chat = await composer({ nonce: 1, ready: false });
  await flush();
  assert.equal(chat.focused(), false);
  await chat.update({ ready: true });
  await flush();
  assert.equal(chat.focused(), true);
  assert.equal(chat.caret(), "side question".length);
  await chat.elsewhere();
  await chat.update({ ready: false });
  await chat.update({ ready: true });
  await flush();
  assert.equal(chat.focused(), false);
});

test("a new nonce requests focus, but the default and unchanged nonce do not", async () => {
  const chat = await composer();
  await flush();
  assert.equal(chat.focused(), false);
  for (const nonce of [1, 2]) {
    await chat.update({ nonce });
    await flush();
    assert.equal(chat.focused(), true);
    await chat.elsewhere();
    await chat.update({ activeId: `rerender-${nonce}` });
    await flush();
    assert.equal(chat.focused(), false);
  }
});

test("search navigation consumes composer focus without leaving the search field", async () => {
  const chat = await composer();
  await chat.search();
  const field = document.activeElement;
  await chat.update({ nonce: 1 });
  await flush();
  assert.ok(document.activeElement === field, "focus stays in the search field");

  await chat.elsewhere();
  await chat.update({ ready: false });
  await chat.update({ ready: true });
  await flush();
  assert.equal(chat.focused(), false, "a request search consumed stays consumed");
});

test("loading cancels scheduled focus without consuming its request", async () => {
  const chat = await composer({ nonce: 1 });
  await chat.update({ ready: false });
  await flush();
  assert.equal(chat.focused(), false);
  await chat.update({ ready: true });
  await flush();
  assert.equal(chat.focused(), true);
});

test("unsuccessful focus leaves the request pending", async () => {
  const chat = await composer({ nonce: 1, inert: true });
  await flush();
  assert.equal(chat.focused(), false);
  await chat.update({ inert: false, ready: false });
  await chat.update({ ready: true });
  await flush();
  assert.equal(chat.focused(), true);
});

test("new-chat focus waits for the requested project and new-chat route", async () => {
  const chat = await composer({ activeId: "existing" });
  await act(async () => requestNewChat("other"));
  await flush();
  assert.equal(chat.focused(), false);
  await chat.update({ projectId: "other", activeId: null, ready: false });
  await flush();
  assert.equal(chat.focused(), false);
  await chat.update({ ready: true });
  await flush();
  assert.equal(chat.focused(), true);
});

test("opening a side chat focuses its composer each time, and navigating to one does not", async () => {
  const sideTab = (sessionId) => `?pane=${encodeURIComponent(JSON.stringify({ kind: "side", sessionId }))}`;
  const chat = await renderChat("/projects/p/tasks/s", { sessions: [session("s"), session("side", { sideParentSessionId: "s" })] });
  const focused = () => document.activeElement === document.querySelectorAll('[data-onboarding="composer"]')[1].querySelector(".cm-content");
  const blur = () => act(async () => document.activeElement.blur());
  const open = async () => {
    await act(async () => find(`button[aria-label="${m.side_chat_open()}"]`).click());
    await settle();
    await flush();
  };

  await open();
  assert.equal(focused(), true);
  await blur();
  await open();
  assert.equal(focused(), true);

  await blur();
  await chat.navigate(`/projects/p/tasks/s${sideTab("side")}`);
  await flush();
  assert.equal(focused(), false);
});

import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, deferred, renderChat, session, userMessage } from "./chat.mjs";
import { press, settle } from "./render.mjs";

/** Three exchanges whose replies all mention "warmup". */
const exchanges = (chat) => ["one", "two", "three"].flatMap((id, index) => [
  userMessage(`${chat}-u-${id}`, `Question ${id}`, index ? `${chat}-a-${["one", "two"][index - 1]}` : null),
  assistantMessage(`${chat}-a-${id}`, `The warmup ${id}`, `${chat}-u-${id}`),
]);
const results = (chat) => ({
  hits: ["one", "two", "three"].map((id) => ({
    sessionId: chat, projectId: "p", messageId: `${chat}-a-${id}`, partId: null, spawnPartId: null, role: "assistant", createdAt: 1,
    chatTitle: `Chat ${chat}`, archived: false, onActiveBranch: true, snippet: `The warmup ${id}`,
  })),
  cursor: null, indexing: false, requiresLongerQuery: false,
});
const pane = (descriptor) => `?pane=${encodeURIComponent(JSON.stringify(descriptor))}`;

/** The main chat, or the side chat open in the right pane. */
const main = () => [...document.querySelectorAll(".chat-pane")].find((section) => !section.closest(".right-pane"));
const side = () => document.querySelector(".right-pane .chat-pane");

/** One chat's find box and the transcript it searches. */
function finder(chat, searchRoot = chat) {
  const field = () => chat().querySelector("[role=search] input[role=combobox]");
  return {
    field,
    get expanded() { return field().getAttribute("aria-expanded") === "true"; },
    get status() { return chat().querySelector("[role=search] [role=status]").textContent; },
    /** The transcript row the last opened result jumped to. */
    get target() { return chat().querySelector("[data-search-target=true]")?.dataset.messageId ?? null; },
    article: (id) => chat().querySelector(`[role=article][data-message-id="${id}"]`),
    /** Selects "warmup" in this chat's transcript and opens find, which searches for the selection. */
    async open() {
      const text = [...chat().querySelectorAll(".msg-assistant .md")].flatMap((element) => {
        const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
        const nodes = [];
        while (walker.nextNode()) nodes.push(walker.currentNode);
        return nodes;
      }).find((node) => node.data.includes("warmup"));
      const range = document.createRange();
      range.setStart(text, text.data.indexOf("warmup"));
      range.setEnd(text, text.data.indexOf("warmup") + "warmup".length);
      await act(async () => {
        window.getSelection().removeAllRanges();
        window.getSelection().addRange(range);
      });
      await act(async () => searchRoot().querySelector(`button[aria-label="${m.chat_search_title()}"]`).click());
      await settle();
    },
  };
}
const mainFind = finder(main, () => document.querySelector(".workspace-tools"));
const sideFind = finder(side);

/** Presses `key` on `target` and lets the jump it starts render; reports whether the page claimed the key. */
async function key(name, target, modifiers = {}) {
  const event = await press(name, target, modifiers);
  await settle();
  return event.defaultPrevented;
}

const sessions = [session("s"), session("side", { sideParentSessionId: "s" })];
const transcripts = { s: exchanges("s"), side: exchanges("side") };
const searchRoute = (_, path) => results(new URL(path, location.href).searchParams.get("sessionId") ?? "s");

/** Renders the main chat at `search`, with the side chat as its side conversation, and opens find there. */
async function renderFind(search = "", routes = {}) {
  const chat = await renderChat(`/projects/p/tasks/s${search}`, { sessions, transcripts, routes: { "GET /api/search": searchRoute, ...routes } });
  await mainFind.open();
  return chat;
}

test("find shortcuts keep navigating after focus moves into the transcript", async () => {
  await renderFind();
  assert.equal(await key("Enter", mainFind.field()), true);
  assert.equal(mainFind.target, "s-a-one");

  const transcript = main().querySelector(".chat-thread");
  const reached = [];
  transcript.addEventListener("keydown", (event) => reached.push(event.key));
  for (const [name, modifiers, expected] of [
    ["F3", {}, "s-a-two"],
    ["F3", { shiftKey: true }, "s-a-one"],
    ["g", { metaKey: true }, "s-a-two"],
    ["g", { ctrlKey: true, shiftKey: true }, "s-a-one"],
  ]) {
    await act(async () => mainFind.article(mainFind.target).focus());
    assert.equal(await key(name, document.activeElement, modifiers), true);
    assert.equal(mainFind.target, expected);
  }
  assert.deepEqual(reached, [], "the transcript never sees a find key, which would cancel the jump it starts");
});

test("the main find box yields to side-chat search and handles preview panes", async () => {
  const chat = await renderFind(pane({ kind: "side", sessionId: "side" }));
  await sideFind.open();
  assert.equal(await key("F3", sideFind.field()), true);
  assert.equal(sideFind.target, "side-a-one");
  assert.equal(mainFind.target, null, "the main find box leaves a side chat's keys to its own search");
  assert.equal(mainFind.expanded, true);

  await act(async () => main().querySelector(`[role=search] button[aria-label="${m.chat_search_close()}"]`).click());
  await act(async () => mainFind.article("s-a-one").focus());
  assert.equal(await key("F3", document.activeElement), false);
  assert.equal(sideFind.status, "1 of 3", "the side chat's search leaves the main chat's keys alone");

  await chat.unmount();
  await renderFind(pane({ kind: "home", view: "experiments" }));
  const preview = document.querySelector(".right-pane [data-workspace-content]");
  await act(async () => preview.focus());
  assert.equal(await key("F3", preview), true);
  assert.equal(mainFind.target, "s-a-one", "a preview pane has no find box of its own, so the main one answers");
});

test("typing, composition, other modifiers, and inert find boxes retain their keys", async (t) => {
  await renderFind(pane({ kind: "home", view: "experiments" }));
  assert.equal(await key("Enter", mainFind.field()), true);
  await act(async () => mainFind.article("s-a-one").focus());
  for (const [name, modifiers] of [["g", {}], ["f", {}], ["f", { metaKey: true, ctrlKey: true }], ["F3", { isComposing: true }], ["F3", { altKey: true }], ["F3", { ctrlKey: true }]]) {
    assert.equal(await key(name, document.activeElement, modifiers), false, `${name} ${JSON.stringify(modifiers)}`);
  }
  assert.equal(mainFind.target, "s-a-one");
  assert.equal(mainFind.status, "1 of 3");

  // A narrow window covers the chat with the open panel, which leaves it inert.
  const { innerWidth } = window;
  t.after(() => window.happyDOM.setInnerWidth(innerWidth));
  await act(async () => window.happyDOM.setInnerWidth(600));
  await settle();
  assert.ok(mainFind.field().closest("[inert]"));
  assert.equal(await key("F3", document.body), false);
  assert.equal(mainFind.target, "s-a-one");
});

test("the expanded result picker opens its selected result before iterating", async () => {
  await renderFind();
  assert.equal(await key("ArrowDown", mainFind.field()), true);
  assert.equal(mainFind.status, "2 of 3");
  assert.equal(mainFind.expanded, true);
  assert.equal(mainFind.target, null);

  assert.equal(await key("F3", mainFind.field()), true);
  assert.equal(mainFind.target, "s-a-two");
  assert.equal(mainFind.expanded, false);
  assert.equal(await key("F3", mainFind.field(), { shiftKey: true }), true);
  assert.equal(mainFind.target, "s-a-one");
});

test("Mod+F refocuses the existing find box while reading a preview", async () => {
  await renderFind(pane({ kind: "home", view: "experiments" }));
  assert.equal(await key("Enter", mainFind.field()), true);
  assert.equal(mainFind.expanded, false);
  const preview = document.querySelector(".right-pane [data-workspace-content]");
  await act(async () => preview.focus());

  const field = mainFind.field();
  field.setSelectionRange(0, 0);
  assert.equal(await key("f", preview, { metaKey: true }), true);
  assert.ok(document.activeElement === field, "focus returns to the find box");
  assert.deepEqual([field.selectionStart, field.selectionEnd], [0, "warmup".length]);
  assert.equal(mainFind.expanded, true);
});

test("Enter pressed while results catch up opens the first result once they arrive", async () => {
  const reply = deferred();
  let first = true;
  await renderFind("", {
    "GET /api/search": (...request) => {
      if (!first) return searchRoute(...request);
      first = false;
      return reply.promise;
    },
  });
  assert.equal(await key("Enter", mainFind.field()), true);
  assert.equal(mainFind.target, null);

  await reply.resolve(results("s"));
  assert.equal(mainFind.target, "s-a-one");
  assert.equal(mainFind.expanded, false);

  assert.equal(await key("f", mainFind.field(), { metaKey: true }), true);
  await act(async () => main().querySelector(`[role=search] button[aria-pressed]:not([aria-pressed=true])`).click());
  await settle();
  assert.equal(mainFind.status, "1 of 3");
  assert.equal(mainFind.expanded, true, "a queued Enter opens once");
});

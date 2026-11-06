import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { setLocale } from "../src/locale.ts";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, emit, renderChat, session, settle, userMessage } from "./chat.mjs";
import { find, press, type } from "./render.mjs";

const observers = new Set();
globalThis.ResizeObserver = class {
  constructor(callback) { this.callback = callback; this.targets = new Set(); observers.add(this); }
  observe(target) { this.targets.add(target); }
  unobserve(target) { this.targets.delete(target); }
  disconnect() { observers.delete(this); }
};

/** Tells every observer of `element` that it resized. */
const resize = (element) => act(async () => {
  for (const observer of [...observers]) if (observer.targets.has(element)) observer.callback([], observer);
});

/** Twenty rows, each as tall (800px, as every element measures here) as the viewport. */
const history = (id) => Array.from({ length: 20 }, (_, index) => index % 2
  ? assistantMessage(`${id}-${index}`, `Reply ${index}`, `${id}-${index - 1}`)
  : userMessage(`${id}-${index}`, `Question ${index}`, index ? `${id}-${index - 1}` : null));

const thread = () => [...document.querySelectorAll('[role="region"]')].find((region) => region.getAttribute("aria-label")?.startsWith("Chat "));
const rows = () => [...document.querySelectorAll("[data-message-id]")].map((row) => row.dataset.messageId);
const atBottom = () => document.querySelector(`button[aria-label="${m.chat_scroll_to_bottom()}"]`).inert;
async function scrollTo(top) {
  const viewport = thread();
  await act(async () => {
    viewport.scrollTop = top;
    viewport.dispatchEvent(new Event("scroll"));
  });
  await settle();
}

async function renderHistory() {
  const transcripts = { s: history("s"), other: history("other") };
  const chat = await renderChat("/projects/p/tasks/s", { sessions: [session("s"), session("other")], transcripts });
  for (const element of [thread(), thread().firstElementChild]) {
    Object.defineProperty(element, "scrollHeight", { configurable: true, get: () => 16_000 });
  }
  return { chat, transcripts };
}

/** Selects `text` in the transcript, as a reader dragging over it does. */
const select = (text) => act(async () => {
  const selection = window.getSelection();
  selection.removeAllRanges();
  if (!text) return;
  const walker = document.createTreeWalker(thread(), NodeFilter.SHOW_TEXT);
  while (walker.nextNode() && !walker.currentNode.data.includes(text));
  const range = document.createRange();
  range.selectNodeContents(walker.currentNode);
  selection.addRange(range);
});

test("virtual history retains selections and dirty forms, then releases closed forms and absent rows", async () => {
  const { transcripts } = await renderHistory();
  await select("Reply 19");
  await scrollTo(0);
  assert.ok(rows().includes("s-0"));
  assert.ok(rows().includes("s-19"), "a selection outside the viewport stays copyable");
  assert.equal(rows().includes("s-17"), false);
  await select(null);
  await scrollTo(16_000);
  await scrollTo(0);
  assert.equal(rows().includes("s-19"), false);

  await scrollTo(16_000);
  const edit = async () => {
    await act(async () => find(`[data-message-id="s-18"] button[aria-label="${m.chat_panel_edit_and_re_send()}"]`).click());
    await type(find(`[data-message-id="s-18"] textarea`), "Question 18, revised");
    await act(async () => thread().focus());
    await scrollTo(0);
  };
  await edit();
  assert.ok(rows().includes("s-18"), "an unsent edit outside the viewport keeps its draft");
  await act(async () => find(`[data-message-id="s-18"] button`, m.chat_panel_cancel()).click());
  await scrollTo(16_000);
  await scrollTo(0);
  assert.equal(rows().includes("s-18"), false);

  await scrollTo(16_000);
  await edit();
  transcripts.s = transcripts.s.slice(0, 18);
  await emit("resync.required", {});
  assert.equal(rows().includes("s-18"), false);
  assert.ok(rows().includes("s-0"));
});

test("tool labels refresh after stream replacement and locale change", async () => {
  const tool = (status) => assistantMessage("a1", [{ id: "tool-1", type: "tool", tool: "bash", state: { status, input: { command: "ls" } } }], "u1");
  const running = tool("running");
  await renderChat("/projects/p/tasks/s", { sessions: [session("s", { busy: true })], transcripts: { s: [userMessage("u1", "list"), running] } });
  const label = () => document.querySelector('[data-message-id="a1"]').textContent;
  const before = label();
  await emit("chat.message", { sessionId: "s", message: tool("completed") });
  await emit("chat.busy", { sessionId: "s", busy: false });
  const completed = label();
  assert.notEqual(completed, before);
  try {
    await act(async () => setLocale("es"));
    await settle();
    assert.notEqual(label(), completed);
  } finally {
    await act(async () => setLocale("en"));
  }
});

const article = (id) => document.querySelector(`[role="article"][data-message-id="${id}"]`);
const focusedRow = () => document.activeElement?.closest?.("[role=article]")?.dataset.messageId ?? null;
const focus = (id) => act(async () => article(id).focus());

test("a focused article stays mounted outside the viewport only while it holds focus", async () => {
  await renderHistory();
  await focus("s-19");
  await scrollTo(0);
  assert.ok(rows().includes("s-19"));
  await act(async () => thread().focus());
  await settle();
  assert.equal(rows().includes("s-19"), false, "a screen reader moving focus while reading retains nothing");
});

test("an opened disclosure stays open after its row scrolls away and back", async () => {
  const transcript = history("s");
  transcript[19] = assistantMessage("s-19", [{ id: "question", type: "prompt", prompt: { kind: "question", question: "Which color?", resolved: true, approved: false } }], "s-18");
  await renderChat("/projects/p/tasks/s", { transcripts: { s: transcript } });
  Object.defineProperty(thread(), "scrollHeight", { configurable: true, get: () => 16_000 });
  const details = () => document.querySelector('[data-message-id="s-19"] details');
  await act(async () => {
    details().open = true;
    details().dispatchEvent(new Event("toggle"));
  });
  await scrollTo(0);
  assert.equal(details(), null);
  await scrollTo(16_000);
  assert.equal(details().open, true);
});

test("ordinary clicks and typing outside a form retain nothing", async () => {
  await renderHistory();
  await act(async () => article("s-19").dispatchEvent(new MouseEvent("click", { bubbles: true })));
  await act(async () => article("s-19").dispatchEvent(new Event("input", { bubbles: true })));
  await scrollTo(0);
  assert.equal(rows().includes("s-19"), false);
});

test("feed keys move focus between articles and leave the feed", async () => {
  await renderHistory();
  const key = async (name, init = {}) => (await press(name, document.activeElement, init)).defaultPrevented;
  await focus("s-18");
  assert.equal(await key("PageUp"), true);
  assert.equal(focusedRow(), "s-17");
  assert.equal(await key("PageDown"), true);
  assert.equal(await key("PageDown"), true);
  assert.equal(focusedRow(), "s-19");
  assert.equal(await key("PageDown"), false, "the last article lets the page scroll");
  for (const init of [{ ctrlKey: true }, { shiftKey: true }]) assert.equal(await key("PageUp", init), false);
  assert.equal(focusedRow(), "s-19");

  assert.equal(await key("Home", { ctrlKey: true }), true);
  assert.equal(document.activeElement === thread(), true);
  await focus("s-19");
  assert.equal(await key("End", { ctrlKey: true }), true);
  assert.equal(document.activeElement === document.querySelector('[data-onboarding="composer"] .cm-content'), true);
});

test("viewport and footer resizing preserve the bottom pin without moving a reader", async () => {
  await renderHistory();
  const viewport = thread();
  assert.equal(atBottom(), true);
  viewport.scrollTop = 0;
  await resize(viewport);
  assert.ok(viewport.scrollTop > 0, "a pinned transcript follows a resize to the end");

  await scrollTo(4_000);
  assert.equal(atBottom(), false);
  const reading = viewport.scrollTop;
  await resize(viewport);
  await resize(viewport.firstElementChild);
  assert.equal(viewport.scrollTop, reading);
  assert.equal(atBottom(), false);
});

test("opening a different conversation pins it to the bottom", async () => {
  const { chat } = await renderHistory();
  await scrollTo(0);
  assert.equal(atBottom(), false);
  await chat.navigate("/projects/p/tasks/other");
  assert.equal(atBottom(), true);
  assert.ok(rows().includes("other-19"));
});

test("the bottom pin follows every scroll except the virtualizer's own", async () => {
  await renderHistory();
  const viewport = thread();
  let height = 16_000;
  Object.defineProperty(viewport, "scrollHeight", { configurable: true, get: () => height });
  const scrolled = () => act(async () => viewport.dispatchEvent(new Event("scroll")));
  /** Rows above the viewport measure taller, so the end moves past where the transcript last scrolled. */
  const grow = () => { height += 4_000; };

  await resize(viewport);
  grow();
  await scrolled();
  const short = viewport.scrollTop;
  await resize(viewport);
  assert.ok(viewport.scrollTop > short, "measuring rows above keeps a streaming chat pinned");

  grow();
  await scrolled();
  await scrolled();
  const reading = viewport.scrollTop;
  await resize(viewport);
  assert.equal(viewport.scrollTop, reading, "a screen reader or focus moving the view unpins it");

  await scrollTo(height - 800);
  grow();
  await resize(viewport);
  assert.equal(viewport.scrollTop, height - 800, "scrolling back to the end pins it again");
});

test("row estimates are computed once per response group", async () => {
  await renderChat("/projects/p/tasks/s", { sessions: [session("s", { busy: true })], transcripts: { s: history("s") } });
  // Estimating a row measures the column it wraps in, so each estimate costs a layout read.
  let reads = 0;
  Object.defineProperty(thread().firstElementChild, "clientWidth", { configurable: true, get: () => { reads++; return 800; } });
  for (const text of ["Reply 19, streaming", "Reply 19, streaming on"]) {
    await emit("chat.message", { sessionId: "s", message: assistantMessage("s-19", text, "s-18") });
  }
  assert.equal(reads, 0, "a streamed reply leaves the unchanged rows' estimates alone");
});

test("a row growing above the view keeps a scroll that hasn't reported yet", async () => {
  await renderHistory();
  const bottom = thread().scrollTop;
  await scrollTo(bottom - 2_500);
  await scrollTo(bottom - 2_000);
  thread().scrollTop = bottom - 2_200;
  const row = find('[data-message-id="s-15"]');
  await act(async () => {
    for (const observer of [...observers]) {
      if (observer.targets.has(row)) observer.callback([{ target: row, borderBoxSize: [{ blockSize: 1_000, inlineSize: 800 }] }], observer);
    }
  });
  await settle();
  assert.equal(thread().scrollTop, bottom - 2_000);
});

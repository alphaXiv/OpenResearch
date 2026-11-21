import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { renderApp, serveApp, settle, streams } from "./render.mjs";

// happy-dom lays nothing out; the rail draws only the rows that fit its height.
for (const size of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"]) {
  Object.defineProperty(HTMLElement.prototype, size, { configurable: true, get: () => 800 });
}

const chat = (id, updatedAt, archived = false) => ({
  id, projectId: "p", harness: "codex", title: `Chat ${id}`, model: null, serviceTier: null, permissionMode: null, planMode: false,
  autonomy: "ask", reasoningLevel: null, archived, busy: false, createdAt: updatedAt, updatedAt,
});

/**
 * Serves the sidebar's lists from `chats`; `totals: false` answers as a server too old to count both sections.
 * `hold(list, later)` can delay a page's answer, which is read when the request arrives.
 */
function serve(chats, { totals = true, patch, hold, pageSize = 50 } = {}) {
  const reads = [];
  const count = (archived) => chats.filter((row) => row.archived === archived).length;
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project", createdAt: Date.now() }] }),
    "GET /api/chat/sessions": (_, url) => {
      const params = new URL(url, location.origin).searchParams;
      if (params.get("scope") !== "sidebar") return chats.filter((row) => row.projectId === params.get("projectId"));
      const archived = params.get("archived") === "true";
      reads.push(archived ? "done" : "inbox");
      const listed = chats.filter((row) => row.archived === archived).sort((a, b) => b.updatedAt - a.updatedAt);
      const before = Number(params.get("beforeUpdatedAt") ?? Infinity);
      const sessions = listed.filter((row) => row.updatedAt < before).slice(0, pageSize);
      const next = listed.length > listed.indexOf(sessions.at(-1)) + 1 ? { updatedAt: sessions.at(-1).updatedAt, id: sessions.at(-1).id } : null;
      const page = { sessions, next, total: listed.length, ...totals && { totals: { active: count(false), archived: count(true) } } };
      const held = hold?.(archived ? "done" : "inbox", params.has("beforeId"));
      return held ? held.then(() => page) : page;
    },
    ...Object.fromEntries(chats.map((row) => [`PATCH /api/chat/sessions/${row.id}`, (init) => patch?.(row, JSON.parse(init.body)) ?? { session: Object.assign(row, JSON.parse(init.body)) }])),
  });
  return { reads, totals: () => ({ active: count(false), archived: count(true) }) };
}

async function openInbox() {
  localStorage.setItem("sidebar-grouping", "inbox");
  localStorage.setItem("sidebar-inbox-collapsed", JSON.stringify(["done"]));
  await renderApp("/projects/p/settings/settings");
}

const rail = () => [...document.querySelectorAll(".session-rail [data-rail-key]")].map((row) => row.dataset.railKey);
const doneHeader = () => [...document.querySelectorAll(".session-rail button[aria-expanded]")].find((button) => button.textContent.startsWith(m.chat_inbox_done()));
const send = (name, data) => act(async () => streams[0].dispatchEvent(new MessageEvent(name, { data: JSON.stringify(data) })));

/** The events a server sends when a chat is marked done somewhere else. */
async function markedDoneElsewhere(row, totals) {
  row.archived = true;
  await send("chat.session", { session: row });
  await send("chat.sidebar", { cards: [{ sessionId: row.id, session: row, atWork: false }], totals: totals() });
  await settle();
}

test("Done reads its chats only once hovered, and counts them from the inbox's answer", { timeout: 20_000 }, async () => {
  const { reads } = serve([chat("a", 3), chat("b", 2), chat("old", 1, true)]);
  try {
    await openInbox();
    assert.deepEqual(reads, ["inbox"]);
    assert.ok(doneHeader().textContent.includes("1"), "Done counts the chat it holds");

    await act(async () => doneHeader().dispatchEvent(new PointerEvent("pointerover", { bubbles: true })));
    await settle();
    assert.deepEqual(reads, ["inbox", "done"]);
  } finally {
    localStorage.clear();
  }
});

test("a chat marked done elsewhere moves on the server's card, without re-reading either list", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 3), chat("b", 2), chat("old", 1, true)];
  const { reads, totals } = serve(chats);
  try {
    await openInbox();
    await markedDoneElsewhere(chats[0], totals);
    assert.ok(!rail().includes("a"));
    assert.ok(doneHeader().textContent.includes("2"));

    await act(async () => doneHeader().click());
    await settle();
    assert.deepEqual(rail().filter((key) => !key.startsWith("section:")), ["b", "a", "old"]);
    assert.deepEqual(reads, ["inbox", "done"]);
  } finally {
    localStorage.clear();
  }
});

test("an older server, which sends no cards, still has the lists re-read when a chat changes", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 3), chat("b", 2), chat("old", 1, true)];
  const { reads } = serve(chats, { totals: false });
  try {
    await openInbox();
    assert.deepEqual(reads.toSorted(), ["done", "inbox"], "Done is read for its count");

    chats[0].archived = true;
    await send("chat.busy", { sessionId: "a", busy: false });
    await settle();
    assert.ok(!rail().includes("a"));
    assert.deepEqual(reads.toSorted(), ["done", "done", "inbox", "inbox"]);
  } finally {
    localStorage.clear();
  }
});

test("a card read before a Done click lands does not put the chat back in the inbox", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 3), chat("b", 2)];
  let land;
  const { totals } = serve(chats, { patch: (row, body) => new Promise((resolve) => { land = () => resolve({ session: Object.assign(row, body) }); }) });
  try {
    await openInbox();
    const stale = { ...chats[0] };
    await act(async () => document.querySelector(`.session-rail [data-rail-key="a"] .session-done-btn`).click());
    await settle();
    assert.ok(!rail().includes("a"));

    await send("chat.sidebar", { cards: [{ sessionId: "a", session: stale, atWork: false }], totals: totals() });
    await settle();
    assert.ok(!rail().includes("a"), "the card from before the click keeps it in Done");
    assert.ok(doneHeader().textContent.includes("1"));

    await act(async () => land());
    await settle();
    await markedDoneElsewhere(chats[0], totals);
    assert.ok(!rail().includes("a"));
  } finally {
    localStorage.clear();
  }
});

test("a list read that started before a card does not undo it", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 3), chat("b", 2)];
  let release;
  const { totals } = serve(chats, { hold: (list) => list === "inbox" && !release ? new Promise((resolve) => { release = resolve; }) : undefined });
  try {
    await openInbox();
    await markedDoneElsewhere(chats[0], totals);
    await act(async () => release());
    await settle();
    assert.deepEqual(rail().filter((key) => !key.startsWith("section:")), ["b"]);
    assert.ok(doneHeader().textContent.includes("1"));
  } finally {
    localStorage.clear();
  }
});

test("a refused Done puts back only its own chat", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 3), chat("b", 2), chat("c", 1)];
  let refuse;
  serve(chats, {
    patch: (row, body) => row.id === "a"
      ? new Promise((resolve) => { refuse = () => resolve(Response.json({ error: "The chat is gone." }, { status: 404 })); })
      : { session: Object.assign(row, body) },
  });
  try {
    await openInbox();
    const done = (id) => act(async () => document.querySelector(`.session-rail [data-rail-key="${id}"] .session-done-btn`).click());
    await done("a");
    await done("b");
    await act(async () => refuse());
    await settle();
    assert.deepEqual(rail().filter((key) => !key.startsWith("section:")), ["a", "c"]);
    assert.ok(doneHeader().textContent.includes("1"));
  } finally {
    localStorage.clear();
  }
});

test("Done counts a move in flight until a card shows it", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 3), chat("b", 2), chat("c", 1)];
  let land;
  const { totals } = serve(chats, { patch: (row, body) => new Promise((resolve) => { land = () => resolve({ session: Object.assign(row, body) }); }) });
  try {
    await openInbox();
    const before = totals();
    await act(async () => document.querySelector(`.session-rail [data-rail-key="a"] .session-done-btn`).click());
    await send("chat.sidebar", { cards: [{ sessionId: "b", session: { ...chats[1], title: "Renamed" }, atWork: false }], totals: before });
    await settle();
    assert.ok(doneHeader().textContent.includes("1"), "a card read before the move still counts it");

    chats[0].archived = true;
    await send("chat.sidebar", { cards: [{ sessionId: "a", session: chats[0], atWork: false }], totals: totals() });
    await settle();
    assert.ok(doneHeader().textContent.includes("1"), "the move's own card is not counted twice");
    await act(async () => land());
    await settle();
    assert.ok(doneHeader().textContent.includes("1"));
  } finally {
    localStorage.clear();
  }
});

test("Done read before a resync is read again when opened, even after a card", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 3), chat("old", 1, true)];
  const { reads, totals } = serve(chats);
  try {
    await openInbox();
    await act(async () => doneHeader().dispatchEvent(new PointerEvent("pointerover", { bubbles: true })));
    await settle();
    await send("resync.required", {});
    await settle();
    await markedDoneElsewhere(chats[0], totals);

    const before = reads.filter((list) => list === "done").length;
    await act(async () => doneHeader().click());
    await settle();
    assert.equal(reads.filter((list) => list === "done").length, before + 1);
  } finally {
    localStorage.clear();
  }
});

test("a card that lands while the next page loads stays applied to the pages before it", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 4), chat("b", 3), chat("c", 2), chat("d", 1)];
  let release;
  const { totals } = serve(chats, { pageSize: 2, hold: (list, later) => list === "inbox" && later && !release ? new Promise((resolve) => { release = resolve; }) : undefined });
  try {
    await openInbox();
    assert.ok(release, "the rail asks for the next page");
    await markedDoneElsewhere(chats[0], totals);
    await act(async () => release());
    await settle();
    assert.deepEqual(rail().filter((key) => !key.startsWith("section:")), ["b", "c", "d"]);
  } finally {
    localStorage.clear();
  }
});

test("a chat past the loaded pages joins Working once it starts work", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 3), chat("b", 2), chat("old", 1)];
  serve(chats, { pageSize: 2, hold: (_, later) => later ? new Promise(() => {}) : undefined });
  try {
    await openInbox();
    assert.ok(!rail().includes("old"));
    await send("chat.sidebar", { cards: [{ sessionId: "old", session: { ...chats[2], busy: true }, atWork: true }], totals: { active: 3, archived: 0 } });
    await settle();
    assert.ok(rail().includes("old"));
  } finally {
    localStorage.clear();
  }
});

test("a refused Done keeps a newer card's changes to that chat", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 3), chat("b", 2)];
  let refuse;
  const { totals } = serve(chats, { patch: () => new Promise((resolve) => { refuse = () => resolve(Response.json({ error: "The chat is gone." }, { status: 404 })); }) });
  try {
    await openInbox();
    await act(async () => document.querySelector(`.session-rail [data-rail-key="a"] .session-done-btn`).click());
    chats[0].title = "Renamed";
    await send("chat.sidebar", { cards: [{ sessionId: "a", session: chats[0], atWork: false }], totals: totals() });
    await act(async () => refuse());
    await settle();
    assert.ok(document.querySelector(`.session-rail [data-rail-key="a"]`)?.textContent.includes("Renamed"));
  } finally {
    localStorage.clear();
  }
});

test("Done read before a resync is read again when opened, even after a Done click", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 3), chat("b", 2), chat("old", 1, true)];
  const { reads } = serve(chats);
  try {
    await openInbox();
    await act(async () => doneHeader().dispatchEvent(new PointerEvent("pointerover", { bubbles: true })));
    await settle();
    await send("resync.required", {});
    await settle();
    await act(async () => document.querySelector(`.session-rail [data-rail-key="a"] .session-done-btn`).click());
    await settle();

    const before = reads.filter((list) => list === "done").length;
    await act(async () => doneHeader().click());
    await settle();
    assert.equal(reads.filter((list) => list === "done").length, before + 1);
  } finally {
    localStorage.clear();
  }
});

test("Done read before a resync is read again when opened, even after a busy change", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 3), chat("old", 1, true)];
  const { reads } = serve(chats);
  try {
    await openInbox();
    await act(async () => doneHeader().dispatchEvent(new PointerEvent("pointerover", { bubbles: true })));
    await settle();
    await send("resync.required", {});
    await settle();
    await send("chat.busy", { sessionId: "old", busy: false });
    await settle();

    const before = reads.filter((list) => list === "done").length;
    await act(async () => doneHeader().click());
    await settle();
    assert.equal(reads.filter((list) => list === "done").length, before + 1);
  } finally {
    localStorage.clear();
  }
});

test("a chat restored into the loaded range shows after the row that ended the page has left", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 5), chat("b", 4), chat("c", 2), chat("r", 4.5, true)];
  const { totals } = serve(chats, { pageSize: 2, hold: (_, later) => later ? new Promise(() => {}) : undefined });
  try {
    await openInbox();
    await markedDoneElsewhere(chats[1], totals);
    chats[3].archived = false;
    await send("chat.sidebar", { cards: [{ sessionId: "r", session: chats[3], atWork: false }], totals: totals() });
    await settle();
    assert.deepEqual(rail().filter((key) => !key.startsWith("section:")).slice(0, 2), ["a", "r"]);
  } finally {
    localStorage.clear();
  }
});

test("two moves in flight shown in one card batch are counted once", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 3), chat("b", 2), chat("c", 1)];
  const lands = [];
  const { totals } = serve(chats, { patch: (row, body) => new Promise((resolve) => { lands.push(() => resolve({ session: Object.assign(row, body) })); }) });
  try {
    await openInbox();
    const done = (id) => act(async () => document.querySelector(`.session-rail [data-rail-key="${id}"] .session-done-btn`).click());
    await done("a");
    await settle();
    await done("b");
    await settle();
    assert.ok(doneHeader().textContent.includes("2"));

    chats[0].archived = true;
    chats[1].archived = true;
    await send("chat.sidebar", { cards: chats.slice(0, 2).map((row) => ({ sessionId: row.id, session: row, atWork: false })), totals: totals() });
    await settle();
    assert.ok(doneHeader().textContent.includes("2") && !doneHeader().textContent.includes("3"));
    for (const land of lands) await act(async () => land());
    await settle();
    assert.ok(doneHeader().textContent.includes("2") && !doneHeader().textContent.includes("3"));
  } finally {
    localStorage.clear();
  }
});

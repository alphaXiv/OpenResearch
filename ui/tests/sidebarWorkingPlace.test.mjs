import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { find, renderApp, serveApp, settle } from "./render.mjs";

// happy-dom lays nothing out; the rail draws only the rows that fit its height.
for (const size of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"]) {
  Object.defineProperty(HTMLElement.prototype, size, { configurable: true, get: () => 800 });
}

const chat = (id, title, updatedAt, busy) => ({
  id, projectId: "p", harness: "codex", title, model: null, serviceTier: null, permissionMode: null, planMode: false,
  autonomy: "ask", reasoningLevel: null, archived: false, busy, createdAt: Date.now(), updatedAt,
});

test("the one-list sidebar puts Working above the inbox, below it, or at the bottom", { timeout: 20_000 }, async () => {
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }),
    // Totals past the loaded page: the folded headers count what the server holds.
    "GET /api/chat/sessions": (_, url) => new URL(url, location.origin).searchParams.get("archived") === "true"
      ? { sessions: [{ ...chat("finished", "Finished chat", 1, false), archived: true }], next: null, total: 12 }
      : { sessions: [chat("waiting", "Waiting chat", 3, false), chat("running", "Running chat", 2, true)], next: null, total: 41 },
  });
  await renderApp("/projects/p/settings/settings");
  const rows = () => [...document.querySelectorAll(".session-rail [data-rail-key]")];
  const rail = () => rows().map((row) => row.dataset.railKey);
  // Each row is drawn below a spacer as tall as the rows above it.
  const workingTop = () => parseFloat(rows().find((row) => row.dataset.railKey === "section:working").previousElementSibling.style.height);
  const click = async (element) => {
    await act(async () => element.click());
    await settle();
  };
  const choose = async (label) => {
    await click(document.querySelector(`button[aria-label="${m.sidebar_customize_view()}"]`));
    await click(find("button", label));
  };

  try {
    assert.deepEqual(rail(), ["section:working", "running", "section:inbox", "waiting", "section:done"], "Working sits above the inbox by default");
    assert.equal(document.querySelector(".session-rail [data-rail-ghost]"), null, "the loading row leaves nothing behind");

    await choose(m.sidebar_working_bottom());
    assert.deepEqual(rail(), ["section:inbox", "waiting", "section:working", "running", "section:done"]);
    assert.ok(workingTop() > 32 + 50, "Working holds at the rail's foot");

    await choose(m.sidebar_working_below());
    assert.deepEqual(rail(), ["section:inbox", "waiting", "section:working", "running", "section:done"]);
    assert.equal(workingTop(), 32 + 50, "Working follows the inbox's header and one card");

    const header = (section) => rows().find((row) => row.dataset.railKey === `section:${section}`).querySelector("button");
    await click(header("inbox"));
    assert.deepEqual(rail(), ["section:inbox", "section:working", "running", "section:done"]);
    assert.match(header("inbox").textContent, /40/, "a folded inbox counts the open chats past the loaded page");
    await click(header("inbox"));

    await click(header("done"));
    assert.ok(rail().includes("finished"));
    assert.match(header("done").textContent, /12/, "an open Done counts every chat it holds");
  } finally {
    localStorage.removeItem("sidebar-working-place");
    localStorage.removeItem("sidebar-inbox-collapsed");
  }
});

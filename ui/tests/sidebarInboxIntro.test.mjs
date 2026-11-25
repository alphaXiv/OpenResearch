import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { find, renderApp, serveApp, settle } from "./render.mjs";

// happy-dom lays nothing out; the rail draws only the rows that fit its height.
for (const size of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"]) {
  Object.defineProperty(HTMLElement.prototype, size, { configurable: true, get: () => 800 });
}

const DAY = 24 * 60 * 60 * 1000;
const chat = (id, age) => ({
  id, projectId: "p", harness: "codex", title: `Chat ${id}`, model: null, serviceTier: null, permissionMode: null, planMode: false,
  reasoningLevel: null, archived: false, busy: false, createdAt: Date.now() - age, updatedAt: Date.now() - age,
});

function serve(chats, projectAge) {
  const patched = [];
  const routes = {
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project", createdAt: Date.now() - projectAge }] }),
    "GET /api/chat/sessions": (_, url) => {
      const archived = new URL(url, location.origin).searchParams.get("archived") === "true";
      const rows = chats.filter((row) => row.archived === archived);
      return url.includes("scope=sidebar") ? { sessions: rows, next: null, total: rows.length } : rows;
    },
  };
  for (const row of chats) {
    routes[`PATCH /api/chat/sessions/${row.id}`] = (init) => {
      patched.push(row.id);
      Object.assign(row, JSON.parse(init.body));
      return { session: row };
    };
  }
  serveApp(routes);
  return patched;
}

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};
const rail = () => [...document.querySelectorAll(".session-rail [data-rail-key]")].map((row) => row.dataset.railKey);

test("someone who used orx before keeps the project list, and their first look at the inbox offers to clear chats idle for days", { timeout: 20_000 }, async () => {
  const chats = [chat("new", DAY), chat("old", 4 * DAY), chat("older", 9 * DAY)];
  const patched = serve(chats, 30 * DAY);
  try {
    await renderApp("/projects/p/settings/settings");
    assert.ok(!rail().includes("section:inbox"), "a returning user starts on the project list");
    assert.equal(localStorage.getItem("sidebar-grouping"), "projects");

    await click(document.querySelector(`button[aria-label="${m.sidebar_customize_view()}"]`));
    await click(find("button", m.sidebar_one_list()));
    const dialog = document.querySelector('[role="dialog"], [role="alertdialog"]');
    assert.ok(dialog?.textContent.includes(m.sidebar_inbox_intro_title()));
    await click(find("button", m.sidebar_inbox_intro_confirm({ count: "2" })));
    assert.deepEqual(patched.sort(), ["old", "older"]);
    assert.deepEqual(rail().filter((key) => !key.startsWith("section:")), ["new"]);

    // Once is enough.
    await click(document.querySelector(`button[aria-label="${m.sidebar_customize_view()}"]`));
    await click(find("button", m.projects_home_projects()));
    await click(document.querySelector(`button[aria-label="${m.sidebar_customize_view()}"]`));
    await click(find("button", m.sidebar_one_list()));
    assert.equal(document.querySelector('[role="dialog"], [role="alertdialog"]'), null);
  } finally {
    localStorage.removeItem("sidebar-grouping");
    localStorage.removeItem("sidebar-inbox-intro");
  }
});

test("a new user starts on the inbox, where a chat's menu marks every chat below it done", { timeout: 20_000 }, async () => {
  const chats = [chat("a", 1000), chat("b", 2000), chat("c", 3000)];
  // Onboarding seeds its demo project hours back.
  const patched = serve(chats, 5 * 60 * 60 * 1000);
  try {
    await renderApp("/projects/p/settings/settings");
    assert.ok(rail().includes("section:inbox"), "a new user starts on the inbox");

    const row = [...document.querySelectorAll(".session-row")].find((element) => element.textContent.includes("Chat a"));
    await act(async () => row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 })));
    await settle();
    await click(find('[role="menuitem"]', m.chat_inbox_mark_below_done()));
    assert.deepEqual(patched.sort(), ["b", "c"]);
  } finally {
    localStorage.removeItem("sidebar-grouping");
    localStorage.removeItem("sidebar-inbox-intro");
  }
});

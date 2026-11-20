import assert from "node:assert/strict";
import test from "node:test";
import { m } from "../src/paraglide/messages.js";
import { find } from "./render.mjs";
import { renderChat, session } from "./chat.mjs";

test("a chat of an unnamed project that fails to load says so and offers a retry", async (t) => {
  localStorage.setItem("sidebar-grouping", "projects");
  t.after(() => localStorage.removeItem("sidebar-grouping"));
  await renderChat("/projects/p/tasks/new", {
    sessions: [session("a")],
    routes: {
      "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project p", updatedAt: 1 }, { id: "u", name: "No project", scratch: true, unnamed: true, updatedAt: 2 }] }),
      "GET /api/projects/activity": () => ({ activity: [{ projectId: "u", totalAgents: 1, lastActivityAt: 2 }] }),
      "GET /api/chat/sessions": (_, url) => url.includes("projectId=u") ? Response.json({ error: "down" }, { status: 500 }) : { sessions: [] },
    },
  });
  assert.ok(find("button", m.app_retry()));
});

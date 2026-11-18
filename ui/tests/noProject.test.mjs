import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { ProjectProposalCard, projectLabel, projectProposal } from "../src/components/NoProject.tsx";
import { orxArgv } from "../src/orxCommand.ts";
import { fitSidebarRows } from "../src/sidebarLayout.ts";
import { m } from "../src/paraglide/messages.js";
import { find, render, serve, settle, type } from "./render.mjs";

test("an agent's orx project create proposes its name and folder", () => {
  assert.deepEqual(projectProposal(orxArgv('orx project create --name "Fib sandbox" --path ~/code/fib')), { name: "Fib sandbox", path: "~/code/fib" });
  assert.deepEqual(projectProposal(orxArgv("orx project create --name=Sweep")), { name: "Sweep", path: "~/OpenResearch/sweep" });
  assert.equal(projectProposal(orxArgv("orx project create")), null);
  assert.equal(projectProposal(orxArgv("orx project view p1")), null);
});

test("a chat without a project reads as No project", () => {
  assert.equal(projectLabel({ name: "No project", scratch: true }), m.no_project());
  assert.equal(projectLabel({ name: "Sweep", scratch: false }), "Sweep");
});

test("chats without a project share one sidebar group", () => {
  const newest = { id: "a", scratch: true };
  const older = { id: "b", scratch: true };
  const other = { id: "c" };
  const rows = [
    { kind: "project", project: other, collapsed: false, busy: false },
    { kind: "chat", project: other, session: { id: "one" } },
    { kind: "chat", project: other, session: { id: "two" } },
    { kind: "project", project: newest, collapsed: false, busy: false },
    { kind: "chat", project: newest, session: { id: "three" } },
    { kind: "chat", project: older, session: { id: "selected" } },
  ];
  // Selecting a chat from another scratch project still keeps the shared heading above it.
  const fitted = fitSidebarRows(rows, 140, "selected");
  assert.deepEqual(fitted.slice(-2), [rows[3], rows[5]]);
});

test("creating the proposed project moves the chat into it", async () => {
  const promoted = [];
  let project = { id: "p1", name: "No project", scratch: true, path: "/data/scratch/p1", createdAt: 1 };
  serve((path, init) => {
    if (path.startsWith("/api/project-path/status")) return { exists: false, directory: false, empty: null };
    if (path === "/api/chat/sessions/chat_1/project" && init.method === "POST") {
      promoted.push(JSON.parse(init.body));
      project = { ...project, name: "Fib sandbox", scratch: false, path: "/home/me/fib", createdAt: 20 };
      return { pending: false, project };
    }
  });
  const view = await render(createElement(ProjectProposalCard, { proposal: { name: "Fib sandbox", path: "~/OpenResearch/fib-sandbox" }, proposedAt: 10, project, session: { id: "chat_1" } }));
  await act(async () => find("button", m.create_project_review()).click());
  await settle();
  const [name, folder] = document.querySelectorAll('[role="dialog"] input');
  assert.equal(name.value, "Fib sandbox");
  assert.equal(folder.value, "~/OpenResearch/fib-sandbox");
  await type(folder, "/home/me/fib");
  await settle();
  await act(async () => find('[role="dialog"] button', m.create_project_confirm()).click());
  await settle();
  assert.deepEqual(promoted, [{ name: "Fib sandbox", path: "/home/me/fib" }]);
  assert.equal(document.querySelector('[role="dialog"]'), null);

  await view.rerender(createElement(ProjectProposalCard, { proposal: { name: "Fib sandbox", path: "~/OpenResearch/fib-sandbox" }, proposedAt: 10, project, session: { id: "chat_1" } }));
  assert.match(document.body.textContent, new RegExp(m.create_project_created()));
  assert.match(document.body.textContent, /\/home\/me\/fib/);
});

test("a folder that already has files blocks creating the project", async () => {
  serve((path) => {
    if (path.startsWith("/api/project-path/status")) return { exists: true, directory: true, empty: false };
  });
  const project = { id: "p1", name: "No project", scratch: true };
  await render(createElement(ProjectProposalCard, { proposal: { name: "Fib", path: "/home/me/full" }, proposedAt: 10, project, session: { id: "chat_1" } }));
  await act(async () => find("button", m.create_project_review()).click());
  await settle();
  assert.equal(document.querySelector('[role="dialog"] [role="alert"]')?.textContent, m.create_project_folder_has_files());
  assert.equal(find('[role="dialog"] button', m.create_project_confirm()).disabled, true);
});

test("a project the chat already had is not the proposed one", async () => {
  const project = { id: "p2", name: "Sweep", scratch: false, path: "/home/me/sweep", createdAt: 5 };
  await render(createElement(ProjectProposalCard, { proposal: { name: "Split", path: "/home/me/split" }, proposedAt: 10, project, session: { id: "chat_2" } }));
  assert.ok(find("button", m.create_project_review()));
  assert.doesNotMatch(document.body.textContent, new RegExp(m.create_project_created()));
});

test("a move waiting for the turn to end shows on the card and can be called off", async () => {
  const cancelled = [];
  serve((path, init) => {
    if (path === "/api/chat/sessions/chat_3/project" && init.method === "DELETE") return cancelled.push(path) && {};
  });
  const project = { id: "p3", name: "No project", scratch: true, createdAt: 1 };
  const session = { id: "chat_3", pendingProject: { name: "Later", path: "/home/me/later" } };
  await render(createElement(ProjectProposalCard, { proposal: { name: "Later", path: "/home/me/later" }, proposedAt: 10, project, session }));
  assert.match(document.body.textContent, /Later/);
  await act(async () => document.querySelector(`[aria-label="${m.create_project_pending_cancel()}"]`).click());
  await settle();
  assert.equal(cancelled.length, 1);
});

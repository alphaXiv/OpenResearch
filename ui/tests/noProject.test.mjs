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
  let project = { id: "p1", name: "No project", scratch: true, path: "/data/scratch/p1" };
  serve((path, init) => {
    if (path.startsWith("/api/project-path/status")) return { exists: false, directory: false, empty: null };
    if (path === "/api/projects/p1/promote" && init.method === "POST") {
      promoted.push(JSON.parse(init.body));
      project = { ...project, name: "Fib sandbox", scratch: false, path: "/home/me/fib" };
      return { project, leftOut: [] };
    }
  });
  const view = await render(createElement(ProjectProposalCard, { proposal: { name: "Fib sandbox", path: "~/OpenResearch/fib-sandbox" }, project, session: { id: "chat_1" } }));
  await act(async () => find("button", m.create_project_review()).click());
  await settle();
  const [name, folder] = document.querySelectorAll('[role="dialog"] input');
  assert.equal(name.value, "Fib sandbox");
  assert.equal(folder.value, "~/OpenResearch/fib-sandbox");
  await type(folder, "/home/me/fib");
  await settle();
  await act(async () => find('[role="dialog"] button', m.create_project_confirm()).click());
  await settle();
  assert.deepEqual(promoted, [{ name: "Fib sandbox", path: "/home/me/fib", sessionId: "chat_1" }]);
  assert.equal(document.querySelector('[role="dialog"]'), null);

  await view.rerender(createElement(ProjectProposalCard, { proposal: { name: "Fib sandbox", path: "~/OpenResearch/fib-sandbox" }, project, session: { id: "chat_1" } }));
  assert.match(document.body.textContent, new RegExp(m.create_project_created()));
  assert.match(document.body.textContent, /\/home\/me\/fib/);
});

test("a folder that already has files blocks creating the project", async () => {
  serve((path) => {
    if (path.startsWith("/api/project-path/status")) return { exists: true, directory: true, empty: false };
  });
  const project = { id: "p1", name: "No project", scratch: true };
  await render(createElement(ProjectProposalCard, { proposal: { name: "Fib", path: "/home/me/full" }, project, session: { id: "chat_1" } }));
  await act(async () => find("button", m.create_project_review()).click());
  await settle();
  await new Promise((resolve) => setTimeout(resolve, 250));
  await settle();
  assert.equal(document.querySelector('[role="dialog"] [role="alert"]')?.textContent, m.create_project_folder_has_files());
  assert.equal(find('[role="dialog"] button', m.create_project_confirm()).disabled, true);
});

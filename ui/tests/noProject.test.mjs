import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { ProjectProposalCard, projectLabel, projectProposal, useCreateProject } from "../src/components/NoProject.tsx";
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

test("a project nobody named reads as a new project", () => {
  assert.equal(projectLabel({ name: "No project", scratch: true, unnamed: true }), m.projects_home_new_project());
  assert.equal(projectLabel({ name: "Tokenizer probes", scratch: true, unnamed: false }), "Tokenizer probes");
  assert.equal(projectLabel({ name: "Sweep", scratch: false }), "Sweep");
});

test("a selected chat of an unnamed project stays in view without a project row", () => {
  const rows = ["one", "two", "selected"].map((id) => ({ kind: "chat", project: { id: `p-${id}`, unnamed: true }, session: { id } }));
  assert.deepEqual(fitSidebarRows(rows, 68, "selected"), [rows[0], rows[2]]);
});

test("creating the proposed project moves the chat into it", async () => {
  const promoted = [];
  let project = { id: "p1", name: "No project", scratch: true, path: "/data/scratch/p1", createdAt: 1 };
  serve((path, init) => {
    if (path.startsWith("/api/project-path/status")) return { exists: false, directory: false, empty: null };
    if (path === "/api/chat/sessions/chat_1/project" && init.method === "POST") {
      promoted.push(JSON.parse(init.body));
      project = { ...project, name: "Fib sandbox", scratch: false, path: "/home/me/fib", proposal: JSON.parse(init.body).proposal, createdAt: 20 };
      return { pending: false, project };
    }
  });
  const view = await render(createElement(ProjectProposalCard, { id: "toolu_fib", proposal: { name: "Fib sandbox", path: "~/OpenResearch/fib-sandbox" }, proposedAt: 10, project, session: { id: "chat_1" } }));
  await act(async () => find("button", m.create_project_review()).click());
  await settle();
  const [name, folder] = document.querySelectorAll('[role="dialog"] input');
  assert.equal(name.value, "Fib sandbox");
  assert.equal(folder.value, "~/OpenResearch/fib-sandbox");
  await type(folder, "/home/me/fib");
  await settle();
  await act(async () => find('[role="dialog"] button', m.create_project_confirm()).click());
  await settle();
  assert.deepEqual(promoted, [{ name: "Fib sandbox", path: "/home/me/fib", project: "p1", scratch: true, proposal: "toolu_fib" }]);
  assert.equal(document.querySelector('[role="dialog"]'), null);

  await view.rerender(createElement(ProjectProposalCard, { id: "toolu_fib", proposal: { name: "Fib sandbox", path: "~/OpenResearch/fib-sandbox" }, proposedAt: 10, project, session: { id: "chat_1" } }));
  assert.match(document.body.textContent, new RegExp(m.create_project_created()));
  assert.match(document.body.textContent, /\/home\/me\/fib/);
});

test("a folder that already has files blocks creating the project", async () => {
  serve((path) => {
    if (path.startsWith("/api/project-path/status")) return { exists: true, directory: true, empty: false };
  });
  const project = { id: "p1", name: "No project", scratch: true };
  await render(createElement(ProjectProposalCard, { id: "toolu_fib", proposal: { name: "Fib", path: "/home/me/full" }, proposedAt: 10, project, session: { id: "chat_1" } }));
  await act(async () => find("button", m.create_project_review()).click());
  await settle();
  assert.equal(document.querySelector('[role="dialog"] [role="alert"]')?.textContent, m.create_project_folder_has_files());
  assert.equal(find('[role="dialog"] button', m.create_project_confirm()).disabled, true);
});

test("a project the chat already had is not the proposed one", async () => {
  const project = { id: "p2", name: "Sweep", scratch: false, path: "/home/me/sweep", createdAt: 5 };
  await render(createElement(ProjectProposalCard, { id: "toolu_split", proposal: { name: "Split", path: "/home/me/split" }, proposedAt: 10, project, session: { id: "chat_2" } }));
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
  await render(createElement(ProjectProposalCard, { id: "toolu_later", proposal: { name: "Later", path: "/home/me/later" }, proposedAt: 10, project, session }));
  assert.match(document.body.textContent, /Later/);
  await act(async () => document.querySelector(`[aria-label="${m.create_project_pending_cancel()}"]`).click());
  await settle();
  assert.equal(cancelled.length, 1);
});

test("creating a project without a proposal says so", async () => {
  const requests = [];
  serve((path, init) => {
    if (path.startsWith("/api/project-path/status")) return { exists: false, directory: false, empty: null };
    if (path === "/api/chat/sessions/chat_8/project" && init.method === "POST") return requests.push(JSON.parse(init.body)) && { pending: true };
  });
  const project = { id: "p8", name: "No project", scratch: true, unnamed: true, createdAt: 1 };
  function TitleBar() {
    const createProject = useCreateProject();
    return createElement("button", { onClick: () => createProject(project, { id: "chat_8" }) }, m.create_project_button());
  }
  await render(createElement(TitleBar));
  await act(async () => find("button", m.create_project_button()).click());
  await settle();
  const [name] = document.querySelectorAll('[role="dialog"] input');
  await type(name, "By hand");
  await settle();
  await act(async () => find('[role="dialog"] button', m.create_project_confirm()).click());
  await settle();
  assert.deepEqual(requests, [{ name: "By hand", path: "~/OpenResearch/by-hand", project: "p8", scratch: true, proposal: "" }]);
});

/** Two proposals in one chat, as the transcript draws them. */
const twoCards = (project, session) => createElement("div", null,
  createElement("section", { "aria-label": "first" }, createElement(ProjectProposalCard, { id: "toolu_a", proposal: { name: "Alpha", path: "/home/me/alpha" }, proposedAt: 10, project, session })),
  createElement("section", { "aria-label": "second" }, createElement(ProjectProposalCard, { id: "toolu_b", proposal: { name: "Beta", path: "/home/me/beta" }, proposedAt: 12, project, session })));
const card = (label) => document.querySelector(`section[aria-label="${label}"]`);

test("of two proposals only the one the user created reads as created", async () => {
  const project = { id: "p4", name: "Beta lab", scratch: false, path: "/home/me/beta-lab", proposal: "toolu_b", createdAt: 20 };
  await render(twoCards(project, { id: "chat_4" }));
  assert.match(card("second").textContent, new RegExp(m.create_project_created()));
  assert.match(card("second").textContent, /Beta lab/);
  assert.match(card("first").textContent, new RegExp(m.create_project_not_created()));
  assert.match(card("first").textContent, /Alpha/);
  assert.equal(card("first").querySelector("button"), null);
});

test("a proposal the user passed over stays uncreated when the chat becomes a project another way", async () => {
  const project = { id: "p5", name: "By hand", scratch: false, path: "/home/me/by-hand", proposal: "", createdAt: 20 };
  await render(twoCards(project, { id: "chat_5" }));
  for (const label of ["first", "second"]) {
    assert.match(card(label).textContent, new RegExp(m.create_project_not_created()));
    assert.doesNotMatch(card(label).textContent, /By hand/);
  }
});

test("a project made before proposals were recorded reads as the one proposed since", async () => {
  const project = { id: "p6", name: "Alpha", scratch: false, path: "/home/me/alpha", createdAt: 20 };
  await render(twoCards(project, { id: "chat_6" }));
  for (const label of ["first", "second"]) assert.match(card(label).textContent, new RegExp(m.create_project_created()));
});

test("a waiting move shows only on the card that asked for it", async () => {
  const project = { id: "p7", name: "No project", scratch: true, createdAt: 1 };
  const pending = (proposal) => ({ id: "chat_7", pendingProject: { name: "Beta", path: "/home/me/beta", proposal, requestedAt: 30 } });
  const view = await render(twoCards(project, pending("toolu_b")));
  const chip = (label) => card(label).querySelector(`[aria-label="${m.create_project_pending_cancel()}"]`);
  assert.ok(chip("second"));
  assert.equal(chip("first"), null);
  assert.ok(find('section[aria-label="first"] button', m.create_project_review()));

  // Asked from the title bar: no card asked for it.
  await view.rerender(twoCards(project, pending("")));
  assert.equal(chip("first"), null);
  assert.equal(chip("second"), null);

  // An older dashboard's request names no proposal, so every card shows it as before.
  await view.rerender(twoCards(project, pending(undefined)));
  assert.ok(chip("first"));
  assert.ok(chip("second"));
});

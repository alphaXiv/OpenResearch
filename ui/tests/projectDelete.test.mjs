import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement, Fragment } from "react";
import { ProjectInfoCard } from "../src/components/ProjectInfoCard.tsx";
import { Toaster } from "../src/components/ui/Toast.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, render, serve, settle } from "./render.mjs";

const project = { id: "p1", name: "Project", artifactsDir: "/files/project" };

/** Opens the delete dialog against a backend whose DELETE answers with `reply`. */
async function openDelete(reply = () => ({})) {
  const calls = [];
  let removed = 0;
  serve((path, init) => {
    if (path.startsWith("/api/projects/p1?") && init.method === "DELETE") {
      calls.push(new URLSearchParams(path.split("?")[1]).get("deleteArtifacts"));
      return reply();
    }
    if (path.startsWith("/api/file-manager")) return { os: "macos", name: "Finder" };
    if (path === "/_orx/runtime") return { kind: "local", version: "test", buildChannel: "stable" };
  });
  await render(createElement(Fragment, null,
    createElement(Toaster),
    createElement(ProjectInfoCard, { project, chatCount: 0, collapsed: false, busy: false, onToggleCollapsed() {}, onRemoved: () => removed++, onNewChat() {}, pinned: false, onPin() {} })));
  await act(async () => document.querySelector(`[aria-label="${m.sidebar_project_details({ name: project.name })}"]`).click());
  await act(async () => find('[role="menuitem"]', m.projects_home_delete_project_action()).click());
  await settle();
  return {
    calls,
    removed: () => removed,
    dialog: () => document.querySelector('[role="dialog"]'),
    checkbox: () => document.querySelector('[role="dialog"] input[type="checkbox"]'),
    confirm: async () => {
      await act(async () => find('[role="dialog"] button', m.projects_home_delete_project_action()).click());
      await settle();
    },
  };
}

test("project deletion keeps artifacts by default and cancellation deletes nothing", async () => {
  const cancelled = await openDelete();
  assert.equal(cancelled.checkbox().checked, false);
  await act(async () => find('[role="dialog"] button', m.projects_home_cancel()).click());
  await settle();
  assert.equal(cancelled.dialog(), null);
  assert.deepEqual(cancelled.calls, []);
});

test("confirming deletes the project and keeps its artifacts", async () => {
  const deletion = await openDelete();
  await deletion.confirm();
  assert.deepEqual(deletion.calls, ["false"]);
  assert.equal(deletion.removed(), 1);
  assert.equal(deletion.dialog(), null);
});

test("artifact deletion requires checkbox opt-in and errors leave the dialog open", async () => {
  const failure = "Keep artifacts to preserve the source folder.";
  const deletion = await openDelete(() => Response.json({ error: failure }, { status: 409 }));
  await act(async () => deletion.checkbox().click());
  await deletion.confirm();
  assert.deepEqual(deletion.calls, ["true"]);
  assert.notEqual(deletion.dialog(), null);
  assert.equal(deletion.removed(), 0);
  assert.equal(document.querySelector('[role="dialog"] [role="alert"]').textContent, failure);
});

test("a failed artifact removal after deletion still closes the dialog and reports it", async () => {
  const artifactsError = "The project was deleted. Could not delete artifacts at /files/project: denied";
  const deletion = await openDelete(() => ({ artifactsError }));
  await deletion.confirm();
  assert.equal(deletion.dialog(), null);
  assert.equal(deletion.removed(), 1);
  assert.match(document.body.textContent, new RegExp(artifactsError));
});

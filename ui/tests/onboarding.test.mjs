import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { ltr } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { find, renderApp, serveApp, settle, type } from "./render.mjs";

/** Opens /projects in the real app against a backend holding `projects` and `uiState`, and answering `routes`. */
async function projectsPage(projects, uiState, routes = {}) {
  serveApp({ "GET /api/projects": () => ({ projects }), "GET /api/settings/ui-state": () => uiState, ...routes });
  return renderApp("/projects");
}

const emptyState = ({ container }) => container.textContent.includes(m.projects_home_no_projects_yet_create_one_to_get_started());

test("a first run without projects opens the onboarding walkthrough", async () => {
  const page = await projectsPage([], { onboardingCompleted: false });
  assert.equal(emptyState(page), false);
  assert.equal(location.pathname, "/projects");
  assert.equal(find("button", m.projects_home_new_project()), null);
});

test("deleting the last project lands on the empty state, not the walkthrough", async () => {
  assert.equal(emptyState(await projectsPage([], { onboardingCompleted: true })), true);
});

test("an existing project resumes instead of walking through onboarding again", async () => {
  const page = await projectsPage([{ id: "p", name: "Project" }], { onboardingCompleted: false });
  assert.equal(emptyState(page), false);
  assert.equal(location.pathname, "/projects/p/tasks/new");
});

test("creating a project from the empty state opens it", async () => {
  const created = { id: "new id", name: "Fresh" };
  const projects = [];
  const page = await projectsPage(projects, { onboardingCompleted: true }, {
    "POST /api/projects": () => {
      projects.push(created);
      return { project: created, githubPublicationError: "publish failed" };
    },
    "GET /api/project-path/status": () => ({ gitVersion: "2.50", resolvedPath: "/home/me/OpenResearch/fresh", exists: false, directory: null, empty: null, initialized: null }),
    "GET /api/settings/projects": () => ({ githubForNewProjects: false, githubDefaultPromptSeen: true, ghInstalled: false, githubAuthenticated: false }),
    "GET /api/github/account": () => ({ login: null }),
    "GET /api/harnesses": () => ({ harnesses: [] }),
    "GET /api/github/project-repo-preview": () => ({ repo: "fresh" }),
  });
  await act(() => find("button", m.projects_home_new_project()).click());
  await settle();
  await type(document.querySelector('[role="dialog"] input:not([type="file"])'), "Fresh");
  await act(() => new Promise((resolve) => setTimeout(resolve, 250)));
  await settle();
  await act(() => document.querySelector('[role="dialog"] form').requestSubmit());
  await settle();

  assert.equal(location.pathname, "/projects/new%20id/settings/projects");
  assert.match(document.body.textContent, /publish failed/);
});

const remote = {
  kind: "ssh", version: "test", buildChannel: "stable", dashboardProtocol: 1,
  session: { id: "gpu", host: "gpu-lab", user: "me", status: "connected", version: "test", dashboardProtocol: 1, gatewayUrl: "http://localhost:4910/", error: null, repairCommand: null, installPaths: null, uiPreferences: { theme: null, locale: null }, canStartNewHost: true },
};

test("a fresh remote goes directly to its own agent setup without the full walkthrough", async () => {
  const page = await projectsPage([], { onboardingCompleted: false, preferredAgent: null }, { "GET /_orx/runtime": () => remote, "GET /api/harnesses": () => ({ harnesses: [] }) });
  assert.match(page.container.textContent, new RegExp(m.remote_agents_title({ host: ltr("gpu-lab") })));
  assert.doesNotMatch(page.container.textContent, new RegExp(m.onboarding_step_1_of_2()));
});

test("a remote with a configured agent skips setup even without the local onboarding flag", async () => {
  const page = await projectsPage([], { onboardingCompleted: false, preferredAgent: { harness: "codex" } }, { "GET /_orx/runtime": () => remote });
  assert.equal(emptyState(page), true);
});

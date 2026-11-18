import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { autoDir } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { find, navigate, renderApp, serveApp, settle, type } from "./render.mjs";

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};

const settingsButton = () => document.querySelector(`button[aria-label="${m.settings_page_settings()}"]`);
const selectedTab = () => document.querySelector('[role="tablist"] [role="tab"][aria-selected="true"]')?.textContent;
const heading = () => document.querySelector(".chat-pane h1")?.textContent;

test("the sidebar's Settings button opens settings, and its tabs switch sections", async () => {
  serveApp({ "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }) });
  await renderApp("/projects/p/tasks/new");
  assert.equal(settingsButton().getAttribute("aria-current"), null);

  await click(settingsButton());
  assert.equal(location.pathname, "/projects/p/settings/settings");
  assert.equal(selectedTab(), m.settings_page_settings());
  assert.equal(settingsButton().getAttribute("aria-current"), "page");

  await click(find('[role="tab"]', m.settings_page_integrations()));
  assert.equal(location.pathname, "/projects/p/settings/integrations");
  assert.equal(heading(), m.settings_page_settings());
  assert.ok(document.body.textContent.includes(m.settings_integrations_intro()));
});

test("links saved by earlier versions open the section that replaced them", async () => {
  serveApp({ "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }) });
  await renderApp("/projects/p/tasks/new");
  for (const [path, tab] of [
    ["/projects/p/settings/harnesses", m.settings_page_harnesses()],
    ["/projects/p/skills", m.settings_page_harnesses()],
    ["/projects/p/settings/projects", m.settings_page_integrations()],
  ]) {
    await navigate(path);
    assert.equal(selectedTab(), tab, path);
    assert.equal(heading(), m.settings_page_settings(), path);
  }
});

test("the Environment tab shows variables for the open project", async () => {
  const project = { id: "p", name: "Training lab" };
  const variable = { key: "ORX_TRAINING_RUN", maskedValue: "••••", inProcessEnv: false };
  serveApp({
    "GET /api/projects": () => ({ projects: [project] }),
    "GET /api/settings/env": (_, url) => {
      const projectId = new URL(url, location.origin).searchParams.get("projectId");
      return { vars: [], projectVars: projectId === project.id ? [variable] : [] };
    },
  });
  await renderApp("/projects/p/tasks/new");

  await click(settingsButton());
  await click(find('[role="tab"]', m.settings_page_environment()));

  assert.ok(find("h2", m.settings_env_project_heading({ project: autoDir(project.name) })));
  assert.ok(find("tr", variable.key)?.textContent.includes(variable.maskedValue));
});

test("the sidebar's chat list can start a chat in the most recently active project", async () => {
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }, { id: "recent", name: "Recent project" }] }),
    "GET /api/projects/activity": () => ({ activity: [{ projectId: "recent", lastActivityAt: 2 }] }),
  });
  await renderApp("/projects/p/settings/settings");

  try {
    await click(document.querySelector(`button[aria-label="${m.chat_panel_filter_sessions()}"]`));
    await click(find("button", m.sidebar_one_list()));
    await click(document.querySelector(`button[aria-label="${m.chat_panel_new_chat()}"]`));
    assert.equal(location.pathname, "/projects/recent/tasks/new");
  } finally {
    localStorage.removeItem("sidebar-grouping");
  }
});

test("a project's settings page renames it and has no global settings tabs", async () => {
  const project = { id: "p", name: "Project", repoPath: "/work/project" };
  const renamed = [];
  serveApp({
    "GET /api/projects": () => ({ projects: [project] }),
    "GET /api/projects/p/git": () => Response.json({ error: "not checked here" }, { status: 500 }),
    "PATCH /api/projects/p": (init) => {
      renamed.push(JSON.parse(init.body));
      return { project: { ...project, ...JSON.parse(init.body) } };
    },
  });
  await renderApp("/projects/p/tasks/new");
  await navigate("/projects/p/settings/project");
  assert.equal(heading(), m.sidebar_project_settings());
  assert.equal(document.querySelector('[role="tablist"]'), null);
  assert.equal(settingsButton().getAttribute("aria-current"), null);
  assert.ok(document.body.textContent.includes("/work/project"));

  const name = document.querySelector(`#${CSS.escape(find("label", m.new_project_form_project_name()).htmlFor)}`);
  const save = find("button", m.common_save());
  assert.equal(save.disabled, true);
  await type(name, "Renamed");
  await click(save);
  assert.deepEqual(renamed, [{ name: "Renamed" }]);
});

import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { clients, find, renderApp, serveApp, settle } from "./render.mjs";

const gitStatus = (github) => ({
  path: "/work/project",
  gitVersion: "2.50.0",
  initialized: true,
  baselineBranch: "main",
  currentBranch: "main",
  clean: true,
  remotes: [],
  identity: { name: null, email: null, nameSource: null, emailSource: null },
  github: { ghInstalled: true, authenticated: true, enabled: false, owner: "", repo: "", url: null, syncStatus: null, ...github },
});
const project = { id: "p", name: "Project" };
const failure = (message) => () => Response.json({ error: message }, { status: 500 });
const pending = () => new Promise(() => {});

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};

/** Opens the GitHub settings for one project; each route below answers a request and records it. */
async function gitSettings({
  git = () => gitStatus(),
  access = () => ({ canPush: true }),
  account = () => ({ login: "example-user" }),
  organizations = () => ({ organizations: ["research-org"] }),
  enabling = () => ({ project, git: gitStatus({ enabled: true }) }),
} = {}) {
  const requests = [];
  const route = (name, reply) => (init) => {
    requests.push(name);
    return reply(init);
  };
  const enabled = [];
  serveApp({
    "GET /api/projects": () => ({ projects: [project] }),
    "GET /api/settings/projects": () => ({ githubForNewProjects: false, githubDefaultPromptSeen: true, ghInstalled: true, githubAuthenticated: true }),
    "GET /api/projects/p/git": route("git", git),
    "GET /api/github/repo-access": route("access", access),
    "GET /api/github/account": route("account", () => account()),
    "GET /api/github/organizations": route("organizations", () => organizations()),
    "POST /api/projects/p/github": (init) => {
      enabled.push(JSON.parse(init.body));
      return enabling();
    },
  });
  await renderApp("/projects/p/settings/projects");
  const projectPicker = document.querySelector(`button[title="${m.settings_github_manage_project()}"]`);
  await click(projectPicker);
  await click([...projectPicker.parentElement.querySelectorAll("button")].find((button) => button.textContent === project.name));
  const enable = () => find("button", m.repository_enable_syncing()) ?? find("button", m.common_checking());
  return {
    enabled,
    requests,
    enable,
    picker: () => document.querySelector(`[role="dialog"][aria-label="${m.settings_github_new_repository_destination()}"]`),
    owner: (name) => find(`[role="dialog"] button`, name),
    retry: () => find("button", m.app_retry()),
  };
}

test("signed-out projects cannot enable GitHub syncing", async () => {
  const view = await gitSettings({ git: () => gitStatus({ authenticated: false }) });
  assert.ok(document.body.textContent.includes(m.settings_page_local_only()));
  assert.ok(!view.enable());
});

test("a local-only project opens a destination picker without creating a repository", async () => {
  const view = await gitSettings();
  assert.ok(!view.picker());
  await click(view.enable());
  assert.ok(view.picker());
  assert.deepEqual(view.enabled, []);
});

test("choosing an organization enables only the selected project under that owner", async () => {
  const view = await gitSettings();
  await click(view.enable());
  assert.ok(view.owner("example-user"));
  await click(view.owner("research-org"));
  assert.deepEqual(view.enabled, [{ organization: "research-org" }]);
  assert.ok(!view.picker());
});

test("choosing the personal account uses the authenticated viewer", async () => {
  const view = await gitSettings();
  await click(view.enable());
  await click(view.owner("example-user"));
  assert.deepEqual(view.enabled, [{}]);
});

test("an existing repository resumes syncing without a destination picker", async () => {
  const view = await gitSettings({ git: () => gitStatus({ owner: "existing-org", repo: "paper" }) });
  await click(view.enable());
  assert.deepEqual(view.enabled, [{}]);
  assert.ok(!view.picker());
});

test("read-only or archived repositories require choosing the replacement owner", async () => {
  const view = await gitSettings({ git: () => gitStatus({ owner: "upstream", repo: "paper" }), access: () => ({ canPush: false }) });
  await click(view.enable());
  assert.deepEqual(view.enabled, []);
  assert.ok(view.picker().textContent.includes(m.settings_create_private_github_repository()));
  await click(view.owner("research-org"));
  assert.deepEqual(view.enabled, [{ organization: "research-org" }]);
});

test("repository access must finish loading before enabling", async () => {
  const view = await gitSettings({ git: () => gitStatus({ owner: "upstream", repo: "paper" }), access: pending });
  assert.equal(view.enable().textContent.trim(), m.common_checking());
  assert.equal(view.enable().disabled, true);
});

test("a failed repository access check disables enabling and offers retry", async () => {
  let fail = true;
  const view = await gitSettings({
    git: () => gitStatus({ owner: "upstream", repo: "paper" }),
    access: () => fail ? failure("GitHub unavailable")() : { canPush: true },
  });
  assert.equal(view.enable().disabled, true);
  assert.ok(document.body.textContent.includes("GitHub unavailable"));
  fail = false;
  await click(view.retry());
  assert.equal(view.requests.filter((name) => name === "access").length, 2);
  assert.equal(view.enable().disabled, false);
});

test("cached owner choices stay visible without a loading placeholder during refresh", async () => {
  let refresh = false;
  const held = (reply) => () => refresh ? pending() : reply();
  const view = await gitSettings({ account: held(() => ({ login: "example-user" })), organizations: held(() => ({ organizations: ["research-org"] })) });
  refresh = true;
  const owners = { predicate: ({ queryKey }) => queryKey[2] === "githubAccount" || queryKey[2] === "githubOrganizations" };
  await act(async () => { for (const client of clients) void client.invalidateQueries(owners); });
  await click(view.enable());
  assert.ok([...clients].some((client) => client.isFetching(owners)));
  assert.ok(!view.picker().querySelector('[aria-busy="true"]'));
  assert.ok(view.owner("research-org"));
});

test("initial discovery reserves space and a failed account lookup offers retry", async () => {
  let release;
  const discovered = new Promise((resolve) => { release = resolve; });
  const view = await gitSettings({
    account: () => discovered.then(() => ({ login: null })),
    organizations: () => discovered.then(() => ({ organizations: [] })),
  });
  await click(view.enable());
  assert.ok(view.picker().querySelector('[aria-busy="true"]'));

  await act(async () => release());
  await settle();
  const before = view.requests.length;
  await click(find('[role="dialog"] button', m.app_retry()));
  assert.deepEqual(view.requests.slice(before).sort(), ["account", "organizations"]);
});

test("a repository-creation error keeps its organization and SSO guidance", async () => {
  const message = "GitHub denied repository creation in 'research-org': HTTP 403: Resource protected by organization SAML enforcement. Authorize GitHub CLI for the organization's SAML SSO, then retry.";
  const view = await gitSettings({ enabling: failure(message) });
  await click(view.enable());
  await click(view.owner("research-org"));
  assert.ok(document.body.textContent.includes(message));
});

test("a push rejected with 403 keeps the write-access explanation", async () => {
  const view = await gitSettings({ enabling: failure("remote: Permission to owner/repo.git denied. The requested URL returned error: 403") });
  await click(view.enable());
  await click(view.owner("research-org"));
  assert.ok(document.body.textContent.includes(m.settings_github_permission_error()));
  assert.ok(!document.body.textContent.includes("requested URL returned error"));
});

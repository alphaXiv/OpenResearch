import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";

const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/components/SettingsPage.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;

// The queries GitTab reads, replaced per test; every other query has no data.
let queries = {};
let clicks = new Map();
let menuOpen = false;
let enableCalls = [];
const text = (children) => React.Children.toArray(children).filter((child) => typeof child === "string").join("").trim();
// Every other app module is a stub: its exports render nothing and return nothing.
const stub = new Proxy({}, { get: (_, name) => (name === "__esModule" ? false : () => null) });
const queryFactories = new Proxy({}, { get: (_, name) => (...args) => ({ queryKey: [name, ...args] }) });
const mocks = {
  "@tanstack/react-query": {
    useQuery: (options) => queries[options?.queryKey?.[0]] ?? { data: undefined },
    useMutation: () => ({}),
    useQueries: () => [],
  },
  "../queries/settings": queryFactories,
  "../queries/projects": queryFactories,
  "../queries/files": queryFactories,
  "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
  "../paraglide/runtime.js": { getLocale: () => "en", isLocale: () => true },
  "../i18n": { ltr: String },
  "./ModelPicker": {
    usePopover: () => ({ open: menuOpen, setOpen: (value) => { menuOpen = value; }, ref: { current: null } }),
  },
  "../api": new Proxy({}, { get: (_, name) => name === "enableProjectGithub" ? async (...args) => {
    enableCalls.push(args);
    return { project: { id: "project-1" }, git: signedIn };
  } : stub[name] }),
  "./ui/cn": { cn: (...names) => names.filter(Boolean).join(" ") },
  // Plain elements, keeping each button's click handler by its label.
  "./ui": new Proxy({}, {
    get: (_, name) => {
      if (name === "Button" || name === "MenuItem") return ({ children, onClick, disabled }) => {
        clicks.set(text(children), onClick);
        return React.createElement("button", { disabled }, children);
      };
      if (name === "Badge") return ({ children }) => React.createElement("span", null, children);
      if (name === "Input") return () => React.createElement("input");
      return stub[name];
    },
  }),
};
const exports = {};
new Function("require", "exports", compiled)(
  (name) => mocks[name] ?? (name.startsWith(".") ? stub : require(name)),
  exports,
);

const gitStatus = (github) => ({
  path: "/work/project",
  gitVersion: "2.50.0",
  initialized: true,
  baselineBranch: "main",
  currentBranch: "main",
  clean: true,
  remotes: [],
  identity: { name: null, email: null, nameSource: null, emailSource: null },
  github: { ghInstalled: true, authenticated: false, enabled: false, owner: "", repo: "", url: null, syncStatus: null, ...github },
});
const signedIn = gitStatus({ authenticated: true });
const signedOut = gitStatus({ authenticated: false });

function renderGitTab({ status, statusError = null, account = { data: { login: "example-user" }, isFetching: false }, organizations = { data: { organizations: ["research-org"] }, isFetching: false }, access = { data: { canPush: true } }, open = false }) {
  const refetches = [];
  menuOpen = open;
  enableCalls = [];
  queries = {
    getProjectGitStatusQuery: { data: status, error: statusError },
    repoAccessQuery: { ...access, refetch: async () => { refetches.push("access"); } },
    githubAccountQuery: { ...account, refetch: async () => { refetches.push("account"); } },
    githubOrganizationsQuery: { ...organizations, refetch: async () => { refetches.push("organizations"); } },
  };
  clicks = new Map();
  const html = renderToStaticMarkup(React.createElement(exports.GitTab, {
    project: { id: "project-1", name: "Project" },
    onProjectUpdate: () => {},
  }));
  return { html, refetches };
}

const buttonLabels = (html) => [...html.matchAll(/<button[^>]*>([\s\S]*?)<\/button>/g)].map((match) => match[1].replaceAll(/<[^>]*>/g, "").trim());
const errors = (html) => [...html.matchAll(/<div class="error">([^<]*)<\/div>/g)].map((match) => match[1]);
const settle = () => new Promise((resolve) => setImmediate(resolve));

test("signed-out projects cannot enable GitHub syncing", () => {
  assert.deepEqual(buttonLabels(renderGitTab({ status: signedOut }).html), []);
});

test("a local-only project opens an owner menu without creating a repository", () => {
  const { html } = renderGitTab({ status: signedIn });
  assert.equal(html.includes('role="menu"'), false);
  assert.equal(html.includes("settings_github_new_repository_destination"), false);
  clicks.get("repository_enable_syncing")({ currentTarget: null });
  assert.equal(menuOpen, true);
  assert.deepEqual(enableCalls, []);
});

test("choosing an organization enables only the selected project under that owner", async () => {
  const { html } = renderGitTab({ status: signedIn, open: true });
  assert.equal(html.includes('role="menu"'), true);
  assert.deepEqual(buttonLabels(html), ["repository_enable_syncing", "example-user", "research-org"]);
  clicks.get("research-org")();
  await settle();
  assert.deepEqual(enableCalls, [["project-1", "research-org"]]);
  assert.equal(menuOpen, false);
});

test("choosing the personal account uses the authenticated viewer", async () => {
  renderGitTab({ status: signedIn, open: true });
  clicks.get("example-user")();
  await settle();
  assert.deepEqual(enableCalls, [["project-1", undefined]]);
});

test("an existing repository resumes syncing without a destination menu", async () => {
  renderGitTab({ status: gitStatus({ authenticated: true, owner: "existing-org", repo: "paper" }) });
  clicks.get("repository_enable_syncing")({ currentTarget: null });
  await settle();
  assert.deepEqual(enableCalls, [["project-1", undefined]]);
  assert.equal(menuOpen, false);
});

test("read-only or archived repositories require choosing the replacement owner", async () => {
  const status = gitStatus({ authenticated: true, owner: "upstream", repo: "paper" });
  renderGitTab({ status, access: { data: { canPush: false } } });
  clicks.get("repository_enable_syncing")({ currentTarget: null });
  assert.equal(menuOpen, true);
  assert.deepEqual(enableCalls, []);
  const { html } = renderGitTab({ status, access: { data: { canPush: false } }, open: true });
  assert.ok(html.includes("settings_create_private_github_repository"));
  clicks.get("research-org")();
  await settle();
  assert.deepEqual(enableCalls, [["project-1", "research-org"]]);
});

test("repository access must finish loading before enabling and failures offer retry", async () => {
  const status = gitStatus({ authenticated: true, owner: "upstream", repo: "paper" });
  const pending = renderGitTab({ status, access: { data: undefined } });
  assert.ok(pending.html.includes('disabled=""'));
  assert.ok(buttonLabels(pending.html).includes("common_checking"));
  const failed = renderGitTab({ status, access: { error: new Error("GitHub unavailable") } });
  assert.ok(failed.html.includes('disabled=""'));
  assert.ok(failed.html.includes("GitHub unavailable"));
  clicks.get("app_retry")();
  await settle();
  assert.deepEqual(failed.refetches, ["access"]);
});

test("cached owner choices stay visible without a loading placeholder during refresh", () => {
  const { html } = renderGitTab({ status: signedIn, open: true,
    account: { data: { login: "example-user" }, isFetching: true },
    organizations: { data: { organizations: ["research-org"] }, isFetching: true } });
  assert.equal(html.includes('aria-busy="true"'), false);
  assert.ok(buttonLabels(html).includes("research-org"));
});

test("initial discovery reserves space and a failed account lookup offers retry", async () => {
  const loading = renderGitTab({ status: signedIn, open: true,
    account: { data: undefined, isFetching: true }, organizations: { data: undefined, isFetching: true } });
  assert.equal(loading.html.includes('aria-busy="true"'), true);
  const { refetches } = renderGitTab({ status: signedIn, open: true,
    account: { data: { login: null }, isFetching: false }, organizations: { data: undefined, isFetching: false } });
  clicks.get("app_retry")();
  await settle();
  assert.deepEqual(refetches, ["account", "organizations"]);
});

test("a repository-creation error keeps its organization and SSO guidance", () => {
  const message = "GitHub denied repository creation in 'research-org': HTTP 403: Resource protected by organization SAML enforcement. Authorize GitHub CLI for the organization's SAML SSO, then retry.";
  const { html } = renderGitTab({ status: signedIn, statusError: new Error(message) });

  assert.deepEqual(errors(html), [message.replaceAll("'", "&#x27;")]);
});

test("a push rejected with 403 keeps the write-access explanation", () => {
  const { html } = renderGitTab({ status: signedIn, statusError: new Error("remote: Permission to owner/repo.git denied. The requested URL returned error: 403") });

  assert.deepEqual(errors(html), ["settings_github_permission_error"]);
});

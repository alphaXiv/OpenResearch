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
const text = (children) => React.Children.toArray(children).filter((child) => typeof child === "string").join("");
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
  "./ui/cn": { cn: (...names) => names.filter(Boolean).join(" ") },
  // Plain elements, keeping each button's click handler by its label.
  "./ui": new Proxy({}, {
    get: (_, name) => {
      if (name === "Button") return ({ children, onClick }) => {
        clicks.set(text(children), onClick);
        return React.createElement("button", null, children);
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

function renderGitTab({ status, statusError = null, account = { data: undefined, isFetching: false }, refetchedStatus = status }) {
  const refetches = [];
  queries = {
    getProjectGitStatusQuery: {
      data: status,
      error: statusError,
      refetch: async () => { refetches.push("status"); return { data: refetchedStatus }; },
    },
    githubAccountQuery: { ...account, refetch: async () => { refetches.push("account"); return { data: account.data }; } },
  };
  clicks = new Map();
  const html = renderToStaticMarkup(React.createElement(exports.GitTab, {
    project: { id: "project-1", name: "Project" },
    onProjectUpdate: () => {},
    remote: false,
  }));
  return { html, refetches };
}

const buttonLabels = (html) => [...html.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map((match) => match[1]);
const errors = (html) => [...html.matchAll(/<div class="error">([^<]*)<\/div>/g)].map((match) => match[1]);
const settle = () => new Promise((resolve) => setImmediate(resolve));

test("checking GitHub after signing in refreshes the account cached while signed out", async () => {
  const { refetches } = renderGitTab({ status: signedOut, refetchedStatus: signedIn });

  clicks.get("settings_check_again")();
  await settle();

  assert.deepEqual(refetches, ["status", "account"]);
});

test("checking GitHub while still signed out leaves the account alone", async () => {
  const { refetches } = renderGitTab({ status: signedOut });

  clicks.get("settings_check_again")();
  await settle();

  assert.deepEqual(refetches, ["status"]);
});

test("a finished account lookup without a login offers a retry", async () => {
  const { html, refetches } = renderGitTab({ status: signedIn, account: { data: { login: null }, isFetching: false } });

  assert.deepEqual(buttonLabels(html), [
    "settings_github_account_unavailable",
    "settings_github_destination_organization",
    "app_retry",
    "repository_enable_syncing",
  ]);
  clicks.get("app_retry")();
  await settle();
  assert.deepEqual(refetches, ["account"]);
});

test("an account lookup in flight shows progress without a retry", () => {
  const { html } = renderGitTab({ status: signedIn, account: { data: undefined, isFetching: true } });

  assert.deepEqual(buttonLabels(html), [
    "settings_github_resolving_account",
    "settings_github_destination_organization",
    "repository_enable_syncing",
  ]);
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

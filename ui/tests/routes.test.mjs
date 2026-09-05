import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import * as routing from "@tanstack/react-router";
import * as react from "react";
import * as jsx from "react/jsx-runtime";
import ts from "typescript";
import * as workspace from "../src/workspaceState.ts";

// Run complete route modules with UI-only dependencies stubbed; route definitions stay real.
function loadModule(filename, api = {}, remembered = null) {
  const cache = new Map();
  function load(url) {
    if (cache.has(url.href)) return cache.get(url.href);
    const output = ts.transpileModule(readFileSync(url, "utf8"), {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
    }).outputText;
    const exports = {};
    cache.set(url.href, exports);
    new Function("require", "exports", output)((id) => {
      if (id === "@tanstack/react-router") return routing;
      if (id === "react") return react;
      if (id.endsWith("/useProjectWorkspace")) return { getCachedProjectWorkspace: () => undefined };
      if (id === "react/jsx-runtime") return jsx;
      if (id.endsWith("/workspaceState")) return workspace;
      if (id.endsWith("/workspacePersistence")) return { getRememberedGlobalWorkspace: () => remembered };
      if (id === "./api") return { isDemoProjectId: () => false, ...api };
      if (id === "./workspaceTabs") return load(new URL("./workspaceTabs.ts", url));
      if (id === "../App") return { default: () => null };
      if (id === "../RemoteRuntime") return { RuntimeRoot: () => null, useRuntime: () => ({ kind: "local" }) };
      if (id === "../routePages") return { ResumeGlobal: () => null, ResumeProject: () => null, ProjectsPage: () => null };
      if (id.startsWith("./routes/")) return load(new URL(`${id}.tsx`, url));
      throw new Error(`Unexpected dependency: ${id}`);
    }, exports);
    return exports;
  }
  return load(new URL(`../src/${filename}`, import.meta.url));
}

async function match(path) {
  const { routeTree } = loadModule("routeTree.gen.ts");
  const router = routing.createRouter({ routeTree, isServer: false, history: routing.createMemoryHistory({ initialEntries: [path] }) });
  await router.load();
  return router;
}

test("real file routes distinguish task/new, task IDs, project index, and settings", async () => {
  for (const [path, routeId] of [
    ["/", "/"],
    ["/projects", "/projects/"],
    ["/projects/p", "/projects/$projectId/"],
    ["/projects/p/tasks/new", "/projects/$projectId/tasks/new"],
    ["/projects/p/tasks/old", "/projects/$projectId/tasks/$sessionId"],
    ["/projects/p/skills", "/projects/$projectId/skills"],
    ["/projects/p/settings/git", "/projects/$projectId/settings/$tab"],
    ["/remote-launch", "/remote-launch"],
  ]) {
    const router = await match(path);
    assert.equal(router.state.matches.at(-1).routeId, routeId, path);
    assert.equal(router.state.matches.at(-1).status, "success", path);
  }
  for (const path of ["/projects/p/settings/unknown", "/projects/p/tasks/%00"]) {
    const invalid = await match(path);
    assert(invalid.state.matches.some((route) => route.status === "notFound"), path);
  }
});

test("project search validates every pane variant and rejects malformed panes", async () => {
  const panes = [
    { kind: "home", view: "files" },
    { kind: "experiment", experimentId: "experiment", view: "terminal", runId: "run" },
    { kind: "file", path: "notes.md", line: 3 },
    { kind: "code", experimentId: "experiment", branch: "main", view: "changes" },
    { kind: "plan", sessionId: "task", promptId: "prompt" },
    { kind: "subagent", sessionId: "task", spawnPartId: "part" },
  ];
  for (const pane of [...panes, { kind: "unknown" }, "broken", null]) {
    const router = await match(`/projects/p/tasks/task?${new URLSearchParams({ pane: JSON.stringify(pane) })}`);
    assert.deepEqual(router.matchRoutes(router.state.location).at(-1).search.pane, workspace.parsePane(pane));
  }
  const closed = await match("/projects/p/tasks/task");
  assert.equal(closed.state.matches.at(-1).search.pane, undefined);
});

function resumeApi(lastLocation, sessions = [], projects = [{ id: "p" }], tasks = {}) {
  return {
    getUiState: async () => ({ workspace: { lastLocation } }),
    getProjectUiState: async () => ({ version: 1, lastLocation, tasks }),
    listProjects: async () => projects,
    listChatSessions: async () => sessions,
  };
}

test("global resume preserves settings and archived task links, and terminates stale redirects at home", async () => {
  for (const [location, sessions, expected] of [
    ["/projects/p/settings/storage", [], "/projects/p/settings/storage"],
    ["/projects/p/tasks/old", [{ id: "old", projectId: "p", archived: true }], "/projects/p/tasks/old"],
    ["/projects/p/tasks/deleted", [], "/projects"],
    ["/projects/deleted/tasks/new", [], "/projects"],
    ["/projects/p", [], "/projects"],
    ["//elsewhere.test", [], "/projects"],
    [null, [], "/projects"],
  ]) {
    const { globalResumeLocation } = loadModule("routeResume.ts", resumeApi(location, sessions));
    assert.equal(await globalResumeLocation(), expected);
  }
});

test("project resume uses API order, keeps remembered pane, and falls back to new when all tasks are archived", async () => {
  const pane = { kind: "file", path: "notes.md" };
  const sessions = [{ id: "archived", archived: true }, { id: "latest", archived: false }, { id: "older", archived: false }];
  const { projectResumeLocation } = loadModule("routeResume.ts", resumeApi(
    "/projects/p/tasks/deleted", sessions, undefined, { latest: { active: pane } },
  ));
  assert.equal(await projectResumeLocation("p"), workspace.taskLocation("p", "latest", pane));
  const empty = loadModule("routeResume.ts", resumeApi("/projects/other/settings/git", [{ id: "archived", archived: true }]));
  assert.equal(await empty.projectResumeLocation("p"), "/projects/p/tasks/new");
});

test("resume uses the current database response and current queued preference; failed reads never become defaults", async () => {
  const saved = "/projects/p/settings/git";
  const api = resumeApi(saved);
  const current = loadModule("routeResume.ts", api, { lastLocation: "/projects/p/skills" });
  assert.equal(await current.globalResumeLocation(), "/projects/p/skills");
  const otherDatabase = loadModule("routeResume.ts", resumeApi(saved, [], [{ id: "other" }]));
  assert.equal(await otherDatabase.globalResumeLocation(), "/projects");
  const failed = loadModule("routeResume.ts", {
    ...api,
    getUiState: async () => { throw new Error("offline"); },
    getProjectUiState: async () => { throw new Error("offline"); },
  });
  await assert.rejects(failed.globalResumeLocation(), /offline/);
  await assert.rejects(failed.projectResumeLocation("p"), /offline/);
});


test("malformed pane cleanup removes only the invalid pane and leaves valid descriptors unchanged", () => {
  const { normalizedPaneSearch } = loadModule("routes/projects.$projectId.tsx");
  assert.equal(normalizedPaneSearch("?pane=not-json"), "");
  assert.equal(normalizedPaneSearch("?pane=%7B%7D&other=1"), "?other=1");
  const valid = new URLSearchParams({ pane: JSON.stringify({ kind: "home", view: "files" }) });
  assert.equal(normalizedPaneSearch(`?${valid}`), null);
  assert.equal(normalizedPaneSearch(`?${valid}&${valid}`), "");
});

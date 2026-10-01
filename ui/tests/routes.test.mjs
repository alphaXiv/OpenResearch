import * as query from "@tanstack/react-query";
import { queryModules } from "./queryModules.mjs";
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
  const queries = queryModules(api);
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
      if (id === "@tanstack/react-query") return query;
      if (id.startsWith("./queries/")) return queries.load(id.slice("./queries/".length));
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

function resumeEffect(bindings) {
  const source = ts.createSourceFile("routePages.tsx", readFileSync(new URL("../src/routePages.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const resume = source.statements.find(node => ts.isFunctionDeclaration(node) && node.name.text === "Resume");
  const effect = resume.body.statements.find(node => ts.isExpressionStatement(node) && ts.isCallExpression(node.expression) && node.expression.expression.getText(source) === "useEffect");
  assert.ok(effect, "Resume effect must exist");
  const code = ts.transpileModule(`return ${effect.expression.arguments[0].getText(source)}`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  return new Function(...Object.keys(bindings), code)(...Object.values(bindings));
}

for (const projectId of [undefined, "p"]) {
  for (const cancellation of ["observer removed", "write invalidation", "route left"]) {
    test(`${projectId ? "project" : "global"} resume handles ${cancellation} without a navigation error`, async () => {
      const client = new query.QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
      let first = true, signal, attempt = 0, error = null, navigated;
      const api = resumeApi(null);
      const name = projectId ? "getProjectUiState" : "getUiState";
      const read = api[name];
      api[name] = (...args) => {
        if (!first) return read(...args);
        first = false;
        signal = args.at(-1);
        return new Promise(() => {});
      };
      const locations = loadModule("routeResume.ts", api);
      const setup = resumeEffect({ ...locations, client, projectId, isCancelledError: query.isCancelledError,
        setError: value => { error = value; }, setAttempt: update => { attempt = update(attempt); },
        navigate: ({ href }) => { navigated = href; },
      });
      const cleanup = setup();
      const queryKey = client.getQueryCache().findAll().find(entry => entry.queryKey[2] === name).queryKey;
      if (cancellation === "route left") cleanup();
      if (cancellation === "observer removed") {
        const observer = new query.QueryObserver(client, { queryKey, enabled: false });
        observer.subscribe(() => {})();
      } else await client.cancelQueries({ queryKey });
      await new Promise(resolve => setImmediate(resolve));
      assert.equal(signal.aborted, true);
      assert.equal(error, null);
      assert.equal(attempt, cancellation === "route left" ? 0 : 1);
      if (cancellation !== "route left") {
        cleanup();
        const finish = setup();
        await new Promise(resolve => setImmediate(resolve));
        assert.equal(navigated, projectId ? "/projects/p/tasks/new" : "/projects");
        finish();
      } else assert.equal(navigated, undefined);
      client.clear();
    });
  }
}

test("resume still exposes real failures instead of automatically retrying them", async () => {
  const failure = new Error("HTTP 503");
  let error, attempts = 0;
  const setup = resumeEffect({ projectId: "p", client: {},
    projectResumeLocation: async () => { throw failure; }, globalResumeLocation: async () => "/projects",
    isCancelledError: query.isCancelledError, navigate: () => assert.fail("must not navigate"),
    setError: value => { error = value; }, setAttempt: () => { attempts++; },
  });
  const cleanup = setup();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(error, failure);
  assert.equal(attempts, 0);
  cleanup();
});

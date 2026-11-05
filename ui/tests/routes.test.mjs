import assert from "node:assert/strict";
import test from "node:test";
import { QueryObserver } from "@tanstack/react-query";
import { act } from "react";
import { globalResumeLocation, projectResumeLocation } from "../src/routeResume.ts";
import { createWorkspaceRouter } from "../src/router.tsx";
import { linkedPaperId } from "../src/routes/reproduce.$.tsx";
import { normalizedPaneSearch } from "../src/routes/projects.$projectId.tsx";
import { createWorkspace } from "../src/workspace.tsx";
import * as workspace from "../src/workspaceState.ts";
import { clients, navigate, renderApp, serveApp, settle } from "./render.mjs";

async function match(path) {
  const router = createWorkspaceRouter(createWorkspace("routes"), path);
  await router.load();
  return router;
}

/** The query client the app at `path` reads through, found by the app's own request for its saved state. */
const appClient = (family) => [...clients].find((client) => client.getQueryCache().findAll().some((entry) => entry.queryKey[2] === family));

test("real file routes distinguish task/new, task IDs, project index, and settings", async () => {
  for (const [path, routeId] of [
    ["/", "/"],
    ["/projects", "/projects/"],
    ["/projects/p", "/projects/$projectId/"],
    ["/projects/p/tasks/new", "/projects/$projectId/tasks/new"],
    ["/projects/p/tasks/old", "/projects/$projectId/tasks/$sessionId"],
    ["/projects/p/skills", "/projects/$projectId/skills"],
    ["/projects/p/settings/settings", "/projects/$projectId/settings/$tab"],
    ["/remote-launch", "/remote-launch"],
  ]) {
    const router = await match(path);
    assert.equal(router.state.matches.at(-1).routeId, routeId, path);
    assert.equal(router.state.matches.at(-1).status, "success", path);
  }
  for (const path of ["/projects/p/settings/git", "/projects/p/settings/unknown", "/projects/p/tasks/%00"]) {
    const invalid = await match(path);
    assert(invalid.state.matches.some((route) => route.status === "notFound"), path);
  }
});

test("reproduce links carry arXiv and alphaXiv-exclusive ids through the path exactly", async () => {
  for (const [path, id] of [
    ["/reproduce/2401.10000", "2401.10000"],
    ["/reproduce/1706.03762v2", "1706.03762v2"],
    ["/reproduce/hep-th/9901001", "hep-th/9901001"],
    ["/reproduce/2610.learning-spectrum-matched-shellsort", "2610.learning-spectrum-matched-shellsort"],
    ["/reproduce/a/b/c", null],
    ["/reproduce/a%20b", null],
  ]) {
    const router = await match(path);
    const leaf = router.state.matches.at(-1);
    assert.equal(leaf.routeId, "/reproduce/$", path);
    assert.equal(linkedPaperId(leaf.params._splat), id, path);
  }
});

test("a .orx file opened with the app reaches the import page with its path", async () => {
  const router = await match("/open?file=%2FUsers%2Fada%2Ftiny%20lm.orx");
  const leaf = router.state.matches.at(-1);
  assert.equal(leaf.routeId, "/open");
  assert.deepEqual(leaf.search, { file: "/Users/ada/tiny lm.orx" });
});

test("a published link opened with the app reaches the import page with its slug", async () => {
  const router = await match("/open?shared=a1B2c3D4e5F6");
  const leaf = router.state.matches.at(-1);
  assert.equal(leaf.routeId, "/open");
  assert.deepEqual(leaf.search, { shared: "a1B2c3D4e5F6" });
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

/** A backend whose saved locations, chat sessions and projects are these. */
function resumeBackend(lastLocation, sessions = [], projects = [{ id: "p" }], tasks = {}, overrides = {}) {
  serveApp({
    "GET /api/settings/ui-state": () => ({ workspace: { lastLocation } }),
    "GET /api/projects/p/ui-state": () => ({ version: 1, lastLocation, tasks }),
    "GET /api/projects": () => ({ projects }),
    "GET /api/chat/sessions": () => ({ sessions }),
    ...overrides,
  });
}

let local;
const fresh = () => { local = createWorkspace("resume"); };

test("global resume preserves settings and archived task links, and falls back to an available project for stale redirects", async () => {
  for (const [location, sessions, expected] of [
    ["/projects/p/settings/storage", [], "/projects/p/settings/storage"],
    ["/projects/p/tasks/old", [{ id: "old", projectId: "p", archived: true }], "/projects/p/tasks/old"],
    ["/projects/p/tasks/deleted", [], "/projects/p/tasks/new"],
    ["/projects/deleted/tasks/new", [], "/projects/p/tasks/new"],
    ["/projects/p", [], "/projects/p/tasks/new"],
    ["//elsewhere.test", [], "/projects/p/tasks/new"],
    [null, [], "/projects/p/tasks/new"],
  ]) {
    fresh();
    resumeBackend(location, sessions);
    assert.equal(await globalResumeLocation(local), expected, String(location));
  }
});

test("project resume uses API order, keeps remembered pane, and falls back to new when all tasks are archived", async () => {
  const pane = { kind: "file", path: "notes.md" };
  const sessions = [{ id: "archived", archived: true }, { id: "latest", archived: false }, { id: "older", archived: false }];
  fresh();
  resumeBackend("/projects/p/tasks/deleted", sessions, undefined, { latest: { active: pane } });
  assert.equal(await projectResumeLocation("p", local), workspace.taskLocation("p", "latest", pane));
  fresh();
  resumeBackend("/projects/other/settings/git", [{ id: "archived", archived: true }]);
  assert.equal(await projectResumeLocation("p", local), "/projects/p/tasks/new");
});

test("resume uses the current database response and current queued preference; failed reads never become defaults", async () => {
  const saved = "/projects/p/settings/git";
  fresh();
  resumeBackend(saved);
  local.persistence.globalWorkspaceWriter.queue({ lastLocation: "/projects/p/skills" });
  assert.equal(await globalResumeLocation(local), "/projects/p/skills");
  fresh();
  resumeBackend(saved, [], [{ id: "other" }]);
  assert.equal(await globalResumeLocation(local), "/projects/other/tasks/new");
  fresh();
  const offline = () => Response.json({ error: "offline" }, { status: 503 });
  resumeBackend(saved, [], undefined, {}, { "GET /api/settings/ui-state": offline, "GET /api/projects/p/ui-state": offline });
  await assert.rejects(globalResumeLocation(local), /offline/);
  await assert.rejects(projectResumeLocation("p", local), /offline/);
});

test("malformed pane cleanup removes only the invalid pane and leaves valid descriptors unchanged", () => {
  assert.equal(normalizedPaneSearch("?pane=not-json"), "");
  assert.equal(normalizedPaneSearch("?pane=%7B%7D&other=1"), "?other=1");
  const valid = new URLSearchParams({ pane: JSON.stringify({ kind: "home", view: "files" }) });
  assert.equal(normalizedPaneSearch(`?${valid}`), null);
  assert.equal(normalizedPaneSearch(`?${valid}&${valid}`), "");
});

for (const projectId of [undefined, "p"]) {
  for (const cancellation of ["observer removed", "write invalidation", "route left"]) {
    test(`${projectId ? "project" : "global"} resume handles ${cancellation} without a navigation error`, async () => {
      let first = true;
      let signal;
      const read = projectId ? "GET /api/projects/p/ui-state" : "GET /api/settings/ui-state";
      const saved = projectId ? { version: 1, lastLocation: null, tasks: {} } : { workspace: { lastLocation: null } };
      resumeBackend(null, [], undefined, {}, {
        [read]: (init) => {
          if (!first) return saved;
          first = false;
          signal = init.signal;
          return new Promise(() => {});
        },
      });
      await renderApp(projectId ? `/projects/${projectId}` : "/");
      const family = projectId ? "getProjectUiState" : "getUiState";
      const queryClient = appClient(family);
      const queryKey = queryClient.getQueryCache().findAll().find((entry) => entry.queryKey[2] === family).queryKey;
      if (cancellation === "route left") await navigate("/projects/p/skills");
      if (cancellation === "observer removed") new QueryObserver(queryClient, { queryKey, enabled: false }).subscribe(() => {})();
      else await act(() => queryClient.cancelQueries({ queryKey }));
      await settle();

      assert.equal(signal.aborted, true);
      assert.equal(location.pathname, cancellation === "route left" ? "/projects/p/skills" : "/projects/p/tasks/new");
    });
  }
}

test("resume still exposes real failures instead of automatically retrying them", async () => {
  let attempts = 0;
  resumeBackend(null, [], undefined, {}, {
    "GET /api/projects/p/ui-state": () => { attempts++; return Response.json({ error: "HTTP 503" }, { status: 503 }); },
  });
  const { container } = await renderApp("/projects/p");
  const afterRender = attempts;
  await settle();
  // The resume loader and the workspace hook each ask once; a failed query is not cached between them.
  assert.ok(afterRender <= 2);
  assert.equal(attempts, afterRender);
  assert.equal(appClient("getProjectUiState").isFetching(), 0);
  assert.match(container.textContent, /HTTP 503/);
  assert.equal(location.pathname, "/projects/p");
});

import assert from "node:assert/strict";
import test from "node:test";
import { queryModules } from "./queryModules.mjs";

function machine(t, id) {
  const data = queryModules(undefined, { identity: id, basePath: `/_orx/workspaces/${id}` });
  t.after(() => data.client.clear());
  return data;
}

const deferred = () => {
  let resolve;
  const promise = new Promise((yes) => { resolve = yes; });
  return { promise, resolve };
};

test("queries bind their keys and destinations before another machine is selected", async (t) => {
  const requests = [];
  t.mock.method(globalThis, "fetch", async (url) => {
    requests.push(url);
    return Response.json({ projects: [{ id: "same-project-id", name: url.includes("/alpha/") ? "Alpha" : "Beta" }] });
  });
  const alpha = machine(t, "alpha"), beta = machine(t, "beta");
  const options = alpha.load("projects").listProjectsQuery();
  const betaOptions = beta.load("projects").listProjectsQuery();
  await beta.client.fetchQuery(betaOptions);
  await alpha.client.fetchQuery(options);
  assert.deepEqual(requests, ["/_orx/workspaces/beta/api/projects", "/_orx/workspaces/alpha/api/projects"]);
  assert.equal(options.queryKey[1], "alpha");
  assert.equal(alpha.client.getQueryData(options.queryKey)[0].name, "Alpha");
  assert.equal(beta.client.getQueryData(betaOptions.queryKey)[0].name, "Beta");
});

test("a multi-step mutation stays on its initiating machine across an await", async (t) => {
  const pending = deferred(), writes = [];
  t.mock.method(globalThis, "fetch", async (url, init) => {
    if (url.endsWith("/ui-state") && init.method !== "POST") return pending.promise;
    writes.push([url, JSON.parse(init.body)]);
    return Response.json({ workspace: null });
  });
  const alpha = machine(t, "alpha"), beta = machine(t, "beta");
  const operation = (async () => {
    await alpha.api.getUiState();
    await alpha.api.updateUiState({ tourCompleted: true });
  })();
  await beta.api.updateUiState({ tourCompleted: false });
  pending.resolve(Response.json({ workspace: null }));
  await operation;
  assert.deepEqual(writes, [
    ["/_orx/workspaces/beta/api/settings/ui-state", { tourCompleted: false }],
    ["/_orx/workspaces/alpha/api/settings/ui-state", { tourCompleted: true }],
  ]);
});

test("a late mutation invalidates only its machine's cache", async (t) => {
  const pending = deferred();
  t.mock.method(globalThis, "fetch", () => pending.promise);
  const alpha = machine(t, "alpha"), beta = machine(t, "beta");
  const alphaKey = alpha.load("settings").getProfileQuery().queryKey;
  const betaKey = beta.load("settings").getProfileQuery().queryKey;
  alpha.client.setQueryData(alphaKey, { name: "Alpha" });
  beta.client.setQueryData(betaKey, { name: "Beta" });
  const write = alpha.api.setProfile({ name: "Updated" });
  pending.resolve(Response.json({ name: "Updated" }));
  await write;
  await new Promise(setImmediate);
  assert.equal(alpha.client.getQueryState(alphaKey).isInvalidated, true);
  assert.equal(beta.client.getQueryState(betaKey).isInvalidated, false);
  assert.equal(beta.client.getQueryData(betaKey).name, "Beta");
});

test("resource links and terminal URLs use the same destination and reject another machine's link", (t) => {
  globalThis.location = { href: "http://localhost:4791/projects", origin: "http://localhost:4791" };
  t.after(() => { delete globalThis.location; });
  const alpha = machine(t, "alpha"), beta = machine(t, "beta");
  for (const value of [alpha.api.projectFileUrl("p", "plot.png"), alpha.api.artifactUrl("p", "plot.png"), alpha.api.chatAttachmentUrl("image.png"), alpha.api.absoluteFileUrl("/tmp/plot.png"), alpha.api.overleafUploadUrl("p", "paper.tex")]) {
    assert.ok(value.startsWith("/_orx/workspaces/alpha/api/"));
    assert.equal(alpha.workspace.url(value), value);
    assert.throws(() => beta.workspace.url(value), /another workspace/);
  }
  assert.throws(() => alpha.workspace.url("http://localhost:4791/_orx/workspaces/beta/api/projects"), /another workspace/);
  assert.equal(alpha.workspace.url("http://localhost:4791/api/projects"), "/_orx/workspaces/alpha/api/projects");
  assert.equal(alpha.workspace.socketUrl("/_orx/ssh/connect").href, "ws://localhost:4791/_orx/workspaces/alpha/_orx/ssh/connect");
  assert.equal(alpha.workspace.url("https://example.com/paper"), "https://example.com/paper");
});

test("retiring a workspace aborts pending work and rejects a late parsed mutation", async (t) => {
  const pending = deferred();
  let signal;
  t.mock.method(globalThis, "fetch", (_url, init) => { signal = init.signal; return pending.promise; });
  const alpha = machine(t, "alpha"), beta = machine(t, "beta");
  const key = beta.load("projects").listProjectsQuery().queryKey;
  beta.client.setQueryData(key, [{ id: "same", name: "Beta" }]);
  const write = alpha.api.updateUiState({ tourCompleted: true });
  alpha.workspace.replaceWorkspace();
  assert.equal(signal.aborted, true);
  pending.resolve(Response.json({ workspace: null }));
  await assert.rejects(write, { name: "AbortError" });
  assert.equal(beta.client.getQueryData(key)[0].name, "Beta");
});

test("engine log reads and run actions stay on their initiating machine", async (t) => {
  const requests = [];
  t.mock.method(globalThis, "fetch", async (url) => {
    requests.push(url);
    return Response.json({ kind: "data", ident: "log", start: 0, data: "", eof: true, len: 0, kept: [] });
  });
  const alpha = machine(t, "alpha"), beta = machine(t, "beta");
  await alpha.api.fetchLogPage("run", 0, 10, null);
  await beta.api.retryEngineHost("gpu-box");
  await alpha.api.abandonRun("run");
  assert.deepEqual(requests, [
    "/_orx/workspaces/alpha/api/runs/run/log?start=0&end=10",
    "/_orx/workspaces/beta/api/compute/hosts/gpu-box/retry",
    "/_orx/workspaces/alpha/api/runs/run/abandon",
  ]);
});

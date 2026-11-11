import assert from "node:assert/strict";
import test from "node:test";
import { mutSaveOverleafToken } from "../src/queries/overleaf.ts";
import { mutDeleteEnvVar, mutRemoveAcpHarness, mutSetComputeDefault, mutSetLitSources, mutSignOutOpenResearch } from "../src/queries/settings.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { serve } from "./render.mjs";

const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return { promise, resolve }; };
const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };
const refused = (error) => Response.json({ error }, { status: 400 });

/** A workspace whose writes wait on `reply` and whose reads answer from `reads`. */
function setup(reads = {}) {
  const workspace = createWorkspace("settings-mutations");
  const reply = deferred();
  const requests = [];
  serve((path, init) => {
    if (!init.method || init.method === "GET") return reads[path];
    requests.push(init.body ? JSON.parse(init.body) : null);
    return reply.promise;
  });
  const set = (options, data) => workspace.queryClient.setQueryData(options.queryKey, data);
  const get = (options) => workspace.queryClient.getQueryData(options.queryKey);
  return { workspace, reply, requests, set, get };
}

const variable = (key) => ({ key, maskedValue: "••••", inProcessEnv: false });

test("a deleted variable leaves the list at once and comes back when the server refuses", async () => {
  const { workspace, reply, set, get } = setup({ "/api/settings/env": { vars: [variable("A"), variable("B")] } });
  const options = workspace.queries.getEnvVarsQuery();
  set(options, [variable("A"), variable("B")]);

  const deleted = mutDeleteEnvVar.runAsHeadlessPromise(workspace, "A", {});
  assert.deepEqual(get(options).map((entry) => entry.key), ["B"]);
  reply.resolve(refused("A is set by the environment."));
  await assert.rejects(deleted, { message: "A is set by the environment." });
  assert.deepEqual(get(options).map((entry) => entry.key), ["A", "B"]);
});

test("a new default destination shows at once and keeps the server's settings", async () => {
  const { workspace, reply, set, get } = setup();
  const options = workspace.queries.getComputeSettingsQuery("p");
  const settings = { defaultBackend: "local", defaultFlavor: null, configuredDefaultBackend: "local", configuredDefaultFlavor: null, targets: [] };
  set(options, settings);

  const saved = mutSetComputeDefault.runAsHeadlessPromise(workspace, { backend: "modal", flavor: "A100", projectId: "p" }, {});
  assert.equal(get(options).configuredDefaultBackend, "modal");
  assert.equal(get(options).defaultFlavor, "A100");
  reply.resolve({ ...settings, defaultBackend: "modal", configuredDefaultBackend: "modal", defaultFlavor: "a100", configuredDefaultFlavor: "a100" });
  await saved;
  assert.equal(get(options).defaultFlavor, "a100");
});

test("two quick source toggles both reach the server, in order, each carrying the other", async () => {
  const { workspace, reply, requests, set, get } = setup();
  const options = workspace.queries.getLitSourcesQuery();
  set(options, { alphaxiv: true, openalex: true, biorxiv: true, pubmed: true });

  const first = mutSetLitSources.runAsHeadlessPromise(workspace, { ...get(options), openalex: false }, {});
  const second = mutSetLitSources.runAsHeadlessPromise(workspace, { ...get(options), pubmed: false }, {});
  assert.deepEqual(get(options), { alphaxiv: true, openalex: false, biorxiv: true, pubmed: false });
  await settle();
  assert.deepEqual(requests, [{ alphaxiv: true, openalex: false, biorxiv: true, pubmed: true }]);
  reply.resolve({ alphaxiv: true, openalex: false, biorxiv: true, pubmed: false });
  await Promise.all([first, second]);
  assert.deepEqual(requests.at(-1), { alphaxiv: true, openalex: false, biorxiv: true, pubmed: false });
});

test("signing out shows the account signed out before the server answers", async () => {
  const { workspace, reply, set, get } = setup();
  const options = workspace.queries.getOpenResearchAccountQuery();
  const signedIn = { loggedIn: true, apiUrl: "https://openresearch.sh", user: { id: "u", email: "a@b.c", firstName: null, lastName: null, alphaxivUrl: null } };
  set(options, signedIn);

  const signedOut = mutSignOutOpenResearch.runAsHeadlessPromise(workspace, {});
  assert.equal(get(options).loggedIn, false);
  reply.resolve(refused("Could not reach openresearch.sh."));
  await assert.rejects(signedOut);
  assert.deepEqual(get(options), signedIn);
});

test("a saved Overleaf token shows in Settings and on every open paper", async () => {
  const { workspace, reply, set, get } = setup();
  const settings = workspace.queries.getOverleafSettingsQuery();
  const paper = workspace.queries.getOverleafStateQuery("p", "paper.tex");
  set(settings, { hasToken: false, hasSession: false });
  set(paper, { hasToken: false, hasSession: false, link: null });

  const saved = mutSaveOverleafToken.runAsHeadlessPromise(workspace, "olp_token", {});
  reply.resolve({ hasToken: true });
  await saved;
  assert.equal(get(settings).hasToken, true);
  assert.equal(get(paper).hasToken, true);
});

test("a removed ACP agent leaves both the agent list and its definitions at once", async () => {
  const { workspace, reply, set, get } = setup({
    "/api/harnesses/acp": [],
    "/api/harnesses": { harnesses: [] },
  });
  const definitions = workspace.queries.listAcpHarnessesQuery();
  const harnesses = workspace.queries.getHarnessesQuery();
  set(definitions, [{ id: "acp:a", name: "Kimi", executable: "kimi", arguments: ["acp"] }]);
  set(harnesses, [{ id: "acp:a" }, { id: "codex" }]);

  const removed = mutRemoveAcpHarness.runAsHeadlessPromise(workspace, "acp:a", {});
  assert.deepEqual(get(definitions), []);
  assert.deepEqual(get(harnesses).map((harness) => harness.id), ["codex"]);
  reply.resolve({ ok: true });
  await removed;
});

import assert from "node:assert/strict";
import { afterEach } from "node:test";
import test from "node:test";
import { QueryObserver } from "@tanstack/react-query";
import { toast } from "sonner";
import { mutUpdateUiState } from "../src/queries/settings.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { serve } from "./render.mjs";

afterEach(() => toast.dismiss());

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

/** A workspace over a server that applies each ui-state write as it arrives and answers when the test says. */
async function setup() {
  let server = { onboardingCompleted: true, hiddenModels: [], modelSectionOrder: ["openai", "anthropic"], favoriteModels: [] };
  const posts = [];
  let offline = false;
  serve((path, init) => {
    if (path !== "/api/settings/ui-state") return undefined;
    if (init.method !== "POST") return structuredClone(server);
    if (offline) return Response.json({ error: "offline" }, { status: 503 });
    server = { ...server, ...JSON.parse(init.body) };
    const snapshot = structuredClone(server);
    return new Promise((resolve) => posts.push(() => resolve(snapshot)));
  });
  const workspace = createWorkspace("test");
  const { queryClient } = workspace;
  const options = workspace.queries.getUiStateQuery();
  await queryClient.fetchQuery(options);
  return {
    posts,
    goOffline: () => { offline = true; },
    /** Saves as the app does, alerting on failure; resolves whether it saved. */
    saveUiState: (change) => new Promise((resolve) => mutUpdateUiState.runWithOptions(workspace, change, { onSettled: ({ status }) => resolve(status === "success") })),
    cache: () => queryClient.getQueryData(options.queryKey),
    set: (update) => queryClient.setQueryData(options.queryKey, update),
    respond: async (index) => { posts[index](); await settle(); await settle(); },
    server: () => server,
    queryClient,
    options,
  };
}

const alerts = () => toast.getToasts().map((item) => item.title);

test("a hide answered before a later reorder keeps the reorder", async () => {
  const app = await setup();

  void app.saveUiState({ hiddenModels: ["a"] });
  void app.saveUiState({ modelSectionOrder: ["anthropic", "openai"] });
  assert.deepEqual(app.cache().modelSectionOrder, ["anthropic", "openai"]);
  // Each write waits for the one before it.
  await settle();
  assert.equal(app.posts.length, 1);

  await app.respond(0);
  assert.deepEqual(app.cache().modelSectionOrder, ["anthropic", "openai"]);
  assert.equal(app.posts.length, 2);

  await app.respond(1);
  assert.deepEqual(app.server().hiddenModels, ["a"]);
  assert.deepEqual(app.server().modelSectionOrder, ["anthropic", "openai"]);
  assert.deepEqual(app.cache().hiddenModels, ["a"]);
  assert.deepEqual(app.cache().modelSectionOrder, ["anthropic", "openai"]);
});

test("quick hides each build on the last, so none comes back", async () => {
  const app = await setup();
  const hide = (id) => app.saveUiState({ hiddenModels: [...app.cache().hiddenModels, id] });

  void hide("a");
  void hide("b");
  await settle();
  await app.respond(0);
  assert.deepEqual(app.cache().hiddenModels, ["a", "b"]);

  void hide("c");
  await app.respond(1);
  assert.deepEqual(app.cache().hiddenModels, ["a", "b", "c"]);
  await app.respond(2);
  assert.deepEqual(app.server().hiddenModels, ["a", "b", "c"]);
  assert.deepEqual(app.cache().hiddenModels, ["a", "b", "c"]);
});

test("a write that fails reverts and says why", async () => {
  const app = await setup();
  const unsubscribe = new QueryObserver(app.queryClient, app.options).subscribe(() => {});
  app.goOffline();

  const saved = app.saveUiState({ favoriteModels: ["claude opus"] });
  assert.deepEqual(app.cache().favoriteModels, ["claude opus"]);
  assert.equal(await saved, false);
  await settle();
  assert.deepEqual(alerts(), ["offline"]);
  assert.deepEqual(app.cache().favoriteModels, []);
  unsubscribe();
});

test("an answer never overwrites a field another writer saved meanwhile", async () => {
  const app = await setup();

  void app.saveUiState({ hiddenModels: ["a"] });
  await settle();
  app.set((state) => ({ ...state, preferredAgent: { harness: "codex", model: "gpt-5.6-sol" } }));

  await app.respond(0);
  assert.deepEqual(app.cache().preferredAgent, { harness: "codex", model: "gpt-5.6-sol" });
  assert.deepEqual(app.cache().hiddenModels, ["a"]);
});

test("a failed write never overwrites a field another writer saved meanwhile", async () => {
  const app = await setup();
  app.goOffline();

  const hidden = app.saveUiState({ hiddenModels: ["a"] });
  app.set((state) => ({ ...state, preferredAgent: { harness: "codex", model: "gpt-5.6-sol" } }));
  assert.equal(await hidden, false);
  assert.deepEqual(app.cache().preferredAgent, { harness: "codex", model: "gpt-5.6-sol" });
  assert.deepEqual(app.cache().hiddenModels, []);
});

test("a failed write reverts only its own fields", async () => {
  const app = await setup();

  void app.saveUiState({ favoriteModels: ["claude opus"] });
  await settle();
  app.goOffline();
  const hidden = app.saveUiState({ hiddenModels: ["a"] });
  await app.respond(0);
  assert.equal(await hidden, false);
  assert.deepEqual(app.cache().favoriteModels, ["claude opus"]);
  assert.deepEqual(app.cache().hiddenModels, []);
});

test("quick writes that all fail restore the server's value, not an earlier unsaved one", async () => {
  const app = await setup();
  app.goOffline();

  const first = app.saveUiState({ hiddenModels: ["a"] });
  const second = app.saveUiState({ hiddenModels: [...app.cache().hiddenModels, "b"] });
  assert.equal(await first, false);
  assert.equal(await second, false);
  await settle();
  assert.deepEqual(app.cache().hiddenModels, []);
});

test("a failed write after a saved one restores the saved value", async () => {
  const app = await setup();

  const first = app.saveUiState({ hiddenModels: ["a"] });
  await settle();
  app.goOffline();
  const second = app.saveUiState({ hiddenModels: [...app.cache().hiddenModels, "b"] });
  await app.respond(0);
  assert.equal(await first, true);
  assert.equal(await second, false);
  await settle();
  assert.deepEqual(app.cache().hiddenModels, ["a"]);
});

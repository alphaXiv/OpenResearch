import assert from "node:assert/strict";
import test from "node:test";
import { mutCancelRun, mutDeleteExperiment, mutSetExperimentArchived } from "../src/queries/projects.ts";
import { mutUpdateUiState } from "../src/queries/settings.ts";
import { mutManageArtifactFile, mutManageProjectFile, mutSaveProjectFile } from "../src/queries/files.ts";
import { createWorkspace } from "../src/workspace.tsx";
import { serve } from "./render.mjs";

const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return { promise, resolve }; };
const refused = (error) => Response.json({ error }, { status: 400 });
const run = (id, experimentId) => ({ id, experimentId, projectId: "p", status: "running", cancelling: false });
const experiment = (id, parentExperimentId = null, archived = false) => ({ id, projectId: "p", parentExperimentId, archived });

/** A workspace whose writes each wait for the test to answer them. */
function setup() {
  const workspace = createWorkspace("project-mutations");
  const replies = [];
  serve((_path, init) => {
    if ((init.method ?? "GET") === "GET") return [];
    const reply = deferred();
    replies.push(reply);
    return reply.promise;
  });
  const seed = (options, data) => workspace.queryClient.setQueryData(options.queryKey, data);
  const read = (options) => workspace.queryClient.getQueryData(options.queryKey);
  return { workspace, replies, seed, read };
}

const uiState = { workspace: null, onboardingCompleted: true, tourCompleted: false, preferredAgent: null, preferredAutonomy: null, nestedDelegation: false };

test("a preference picked twice in a row is not undone by the first reply", async () => {
  const { workspace, replies, seed, read } = setup();
  const options = workspace.queries.getUiStateQuery();
  seed(options, uiState);
  const codex = { harness: "codex", model: null };
  const claude = { harness: "claude", model: "opus" };

  const first = mutUpdateUiState.runAsHeadlessPromise(workspace, { preferredAgent: codex }, {});
  const second = mutUpdateUiState.runAsHeadlessPromise(workspace, { preferredAgent: claude }, {});
  assert.deepEqual(read(options).preferredAgent, claude);

  replies[0].resolve({ ...uiState, preferredAgent: { ...codex, permissionMode: "default" } });
  await first;
  assert.deepEqual(read(options).preferredAgent, claude);
  while (replies.length < 2) await new Promise((resolve) => setTimeout(resolve));
  replies[1].resolve({ ...uiState, preferredAgent: { ...claude, permissionMode: "default" } });
  await second;
  assert.deepEqual(read(options).preferredAgent, { ...claude, permissionMode: "default" });
});

test("deleting an experiment removes it and its runs until the server refuses", async () => {
  const { workspace, replies, seed, read } = setup();
  const experiments = workspace.queries.listExperimentsQuery("p");
  const runs = workspace.queries.listRunsQuery("p");
  seed(experiments, [experiment("a"), experiment("b")]);
  seed(runs, [run("r1", "a"), run("r2", "b")]);

  const deleted = mutDeleteExperiment.runAsHeadlessPromise(workspace, { id: "a", projectId: "p" }, {});
  assert.deepEqual(read(experiments).map((row) => row.id), ["b"]);
  assert.deepEqual(read(runs).map((row) => row.id), ["r2"]);

  replies[0].resolve(refused("Experiment a has a live run."));
  await assert.rejects(deleted, { message: "Experiment a has a live run." });
  assert.deepEqual(read(experiments).map((row) => row.id), ["a", "b"]);
  assert.deepEqual(read(runs).map((row) => row.id), ["r1", "r2"]);
});

test("archiving a region marks the experiments the server names", async () => {
  const { workspace, replies, seed, read } = setup();
  const experiments = workspace.queries.listExperimentsQuery("p");
  seed(experiments, [experiment("a", null, true), experiment("b", "a", true), experiment("c")]);

  const restored = mutSetExperimentArchived.runAsHeadlessPromise(workspace, { id: "b", projectId: "p" }, "region", false, {});
  replies[0].resolve({ ids: ["a", "b"] });
  await restored;
  assert.deepEqual(read(experiments).map((row) => row.archived), [false, false, false]);
});

test("stopping a run shows it stopping wherever the run is listed", async () => {
  const { workspace, seed, read } = setup();
  const runs = workspace.queries.listRunsQuery("p");
  const active = workspace.queries.listChatActiveRunsQuery();
  seed(runs, [run("r1", "a"), run("r2", "a")]);
  seed(active, [{ sessionId: "s", run: run("r1", "a"), experimentName: null }]);

  void mutCancelRun.runAsHeadlessPromise(workspace, { id: "r1", projectId: "p" }, {});
  assert.deepEqual(read(runs).map((row) => row.cancelling), [true, false]);
  assert.equal(read(active)[0].run.cancelling, true);
});

test("renaming an artifact folder moves everything inside it", async () => {
  const { workspace, seed, read } = setup();
  const artifacts = workspace.queries.getArtifactsQuery("p");
  const file = (path) => ({ name: path.split("/").pop(), path, isDir: false, size: 1, modifiedAt: 1 });
  seed(artifacts, {
    dir: "/a",
    truncated: false,
    entries: [
      { name: "figs", path: "figs", isDir: true, size: 0, modifiedAt: 1, children: [file("figs/loss.png"), { name: "old", path: "figs/old", isDir: true, size: 0, modifiedAt: 1, children: [file("figs/old/a.png")] }] },
      file("report.md"),
    ],
  });

  void mutManageArtifactFile.runAsHeadlessPromise(workspace, "p", "figs/old", { action: "rename", newName: "archive" }, {});
  const [figs] = read(artifacts).entries;
  assert.deepEqual(figs.children.map((entry) => entry.path), ["figs/loss.png", "figs/archive"]);
  assert.equal(figs.children[1].name, "archive");
  assert.deepEqual(figs.children[1].children.map((entry) => entry.path), ["figs/archive/a.png"]);

  void mutManageArtifactFile.runAsHeadlessPromise(workspace, "p", "report.md", { action: "delete" }, {});
  assert.deepEqual(read(artifacts).entries.map((entry) => entry.path), ["figs"]);
});

test("renaming and deleting in a chat's worktree updates its file list", async () => {
  const { workspace, seed, read } = setup();
  const tree = workspace.queries.getCodeTreeQuery("p", { sessionId: "s" });
  seed(tree, { root: "worktree", branch: "b", truncated: false, entries: ["a.py", "src/main.py", "src/util.py", "z.py"] });

  void mutManageProjectFile.runAsHeadlessPromise(workspace, "p", "s", "src", { action: "rename", newName: "lib" }, {});
  assert.deepEqual(read(tree).entries, ["a.py", "lib/main.py", "lib/util.py", "z.py"]);
  void mutManageProjectFile.runAsHeadlessPromise(workspace, "p", "s", "z.py", { action: "delete" }, {});
  assert.deepEqual(read(tree).entries, ["a.py", "lib/main.py", "lib/util.py"]);
});

test("a saved file's new version reaches every open view of it", async () => {
  const { workspace, replies, seed, read } = setup();
  const loaded = (version) => ({ source: "checkout", file: { path: "notes.md", content: "old", version, root: "worktree" } });
  const repoTab = workspace.queries.resolvedFileQuery("p", "notes.md", "repo", "s");
  const artifactTab = workspace.queries.resolvedFileQuery("p", "notes.md", "artifacts", "s");
  const otherChat = workspace.queries.resolvedFileQuery("p", "notes.md", "repo", "t");
  for (const options of [repoTab, artifactTab, otherChat]) seed(options, loaded("v1"));

  const saved = mutSaveProjectFile.runAsHeadlessPromise(workspace, "p", "s", "notes.md", "new", "v1", {});
  replies[0].resolve({ ok: true, root: "worktree", bytesWritten: 3, version: "v2" });
  await saved;
  assert.deepEqual([repoTab, artifactTab].map((options) => read(options).file), [
    { path: "notes.md", content: "new", version: "v2", root: "worktree" },
    { path: "notes.md", content: "new", version: "v2", root: "worktree" },
  ]);
  assert.equal(read(otherChat).file.version, "v1");
});

import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import ts from "typescript";

const code = ts.transpileModule(readFileSync(new URL("../src/artifactSearch.ts", import.meta.url), "utf8"), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText;
const exports = {};
new Function("exports", code)(exports);
const { ArtifactSearchSession } = exports;
const job = (status, stage = 0) => ({ id: "same-search", status, stage, result: null, error: null });
const flush = async () => { await Promise.resolve(); await Promise.resolve(); };

test("slow search continues the same job through both warnings without restarting", async t => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let starts = 0, stage = 0;
  const resumes = [], updates = [];
  const session = new ArtifactSearchSession({
    start: async () => { starts++; return job("running"); },
    status: async () => stage < 2 ? job("paused", stage) : { ...job("complete", stage), result: { entries: [], nextCursor: null, incomplete: false } },
    resume: async id => { resumes.push(id); return job("running", ++stage); },
    cancel: async () => {},
  }, update => updates.push(update), error => { throw error; });
  await session.start("report");
  t.mock.timers.tick(250); await flush();
  assert.deepEqual([updates.at(-1).status, updates.at(-1).stage], ["paused", 0]);
  await session.resume();
  t.mock.timers.tick(250); await flush();
  assert.deepEqual([updates.at(-1).status, updates.at(-1).stage], ["paused", 1]);
  await session.resume();
  t.mock.timers.tick(250); await flush();
  assert.equal(updates.at(-1).status, "complete");
  assert.equal(starts, 1);
  assert.deepEqual(resumes, ["same-search", "same-search"]);
  session.dispose();
});

test("query change/unmount cancels late starts and cancellation ignores in-flight results", async t => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let resolveStart, resolveStatus;
  const cancelled = [], updates = [];
  const client = {
    start: () => new Promise(resolve => { resolveStart = resolve; }),
    status: () => new Promise(resolve => { resolveStatus = resolve; }),
    resume: async () => { throw new Error("cancelled searches must not resume"); },
    cancel: async id => { cancelled.push(id); },
  };
  const abandoned = new ArtifactSearchSession(client, update => updates.push(update), error => { throw error; });
  const starting = abandoned.start("old");
  abandoned.dispose();
  resolveStart(job("running")); await starting;
  assert.deepEqual(cancelled, ["same-search"]);
  assert.equal(updates.length, 0);
  const active = new ArtifactSearchSession({ ...client, start: async () => job("running") }, update => updates.push(update), error => { throw error; });
  await active.start("new");
  t.mock.timers.tick(250); await flush();
  active.cancel();
  resolveStatus(job("complete")); await flush();
  await active.resume();
  assert.equal(updates.at(-1).status, "cancelled");
  assert.equal(updates.some(update => update.status === "complete"), false);
  assert.equal(cancelled.length, 2);
});

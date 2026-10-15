import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";

const source = readFileSync(new URL("../src/components/NewProjectForm.tsx", import.meta.url), "utf8");
const chooseFolder = source.match(/  async function chooseLocalFolder\(\) \{[\s\S]*?\n  \}/)[0];

function picker(pickProjectFolder) {
  const state = { failed: false, error: null, picking: false, path: "" };
  const folderPickSeq = { current: 0 };
  const choose = runInNewContext(`(${chooseFolder})`, {
    pickProjectFolder,
    pickingFolder: false,
    folderPickSeq,
    setPickingFolder: (value) => { state.picking = value; },
    setFolderPickerFailed: (value) => { state.failed = value; },
    setError: (value) => { state.error = value; },
    setPath: (value) => { state.path = value; },
    setPathTouched: () => {},
    mode: "folder",
    nameTouched: true,
    queryClient: { invalidateQueries: () => {} },
    getProjectPathStatusQuery: () => ({}),
  });
  return { choose, state, folderPickSeq };
}

test("picker failure reports the error and enables the manual-path fallback", async () => {
  const { choose, state } = picker(async () => { throw "No graphical display"; });
  await choose();
  assert.deepEqual(state, { failed: true, error: "No graphical display", picking: false, path: "" });
  assert.match(source, /mode === "folder" && !remote && !folderPickerFailed/);
});

test("normal cancellation preserves the picker without an error", async () => {
  const { choose, state } = picker(async () => null);
  await choose();
  assert.deepEqual(state, { failed: false, error: null, picking: false, path: "" });
});

test("successful selection still sets the project path", async () => {
  const { choose, state } = picker(async () => "/home/user/project");
  await choose();
  assert.deepEqual(state, { failed: false, error: null, picking: false, path: "/home/user/project" });
});

test("a stale picker error cannot change the current form", async () => {
  let reject;
  const result = picker(() => new Promise((_, fail) => { reject = fail; }));
  const pending = result.choose();
  result.folderPickSeq.current += 1;
  reject("No graphical display");
  await pending;
  assert.equal(result.state.failed, false);
  assert.equal(result.state.error, null);
});

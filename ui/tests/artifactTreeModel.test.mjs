import assert from "node:assert/strict";
import { test } from "node:test";

import { filterArtifactEntries, sortArtifactEntries } from "../src/components/artifactTreeModel.ts";

const file = (name, modifiedAt, children) => ({
  name,
  path: name,
  isDir: children !== undefined,
  size: 10,
  modifiedAt,
  ...(children !== undefined ? { children } : {}),
});

test("name order is the server's order as is", () => {
  const entries = [file("figures", 20, [file("b.png", 35), file("a.png", 5)]), file("Zed.log", 40), file("report.md", 30)];
  assert.equal(sortArtifactEntries(entries, "name"), entries);
});

test("newest and oldest order siblings by modified time, recursively", () => {
  const entries = [file("figures", 20, [file("a.png", 5), file("b.png", 35)]), file("report.md", 30)];
  assert.deepEqual(
    sortArtifactEntries(entries, "newest").map((entry) => entry.name),
    ["figures", "report.md"],
  );
  assert.deepEqual(
    sortArtifactEntries(entries, "oldest").map((entry) => entry.name),
    ["figures", "report.md"],
  );
  const dir = sortArtifactEntries(entries, "newest")[0];
  assert.deepEqual(
    dir.children.map((entry) => entry.name),
    ["b.png", "a.png"],
  );
});

test("sorting never mutates the input or reorders children of the original", () => {
  const inner = [file("b.png", 35), file("a.png", 5)];
  const entries = [file("figures", 20, inner)];
  sortArtifactEntries(entries, "oldest");
  assert.deepEqual(
    entries[0].children.map((entry) => entry.name),
    ["b.png", "a.png"],
  );
});

test("filtering keeps matched files and the folders that contain them", () => {
  const entries = [
    file("figures", 20, [file("accuracy.png", 5), file("loss.png", 35)]),
    file("report.md", 30),
  ];
  const kept = filterArtifactEntries(entries, "acc");
  assert.equal(kept.length, 1);
  assert.equal(kept[0].name, "figures");
  assert.deepEqual(
    kept[0].children.map((entry) => entry.name),
    ["accuracy.png"],
  );
  assert.equal(filterArtifactEntries(entries, "report.md").length, 1);
  assert.equal(filterArtifactEntries(entries, "loss.png")[0].name, "figures");
});

test("filtering trims nested folders down to their matches", () => {
  const entries = [file("outputs", 20, [file("figures", 20, [file("loss.png", 5), file("notes.txt", 5)])])];
  const [outputs] = filterArtifactEntries(entries, "png");
  assert.deepEqual(
    outputs.children[0].children.map((entry) => entry.name),
    ["loss.png"],
  );
});

test("filtering is case-insensitive and an empty query keeps everything", () => {
  const entries = [file("Report.md", 30)];
  assert.equal(filterArtifactEntries(entries, "REPORT").length, 1);
  assert.equal(filterArtifactEntries(entries, "  ").length, 1);
  assert.equal(filterArtifactEntries(entries, "").length, 1);
  assert.equal(filterArtifactEntries(entries, "nomatch").length, 0);
});

test("a folder whose own name matches keeps its whole subtree", () => {
  const entries = [file("sweeps", 20, [file("old.csv", 5), file("new.csv", 35)])];
  const kept = filterArtifactEntries(entries, "sweeps");
  assert.equal(kept[0].children.length, 2);
});

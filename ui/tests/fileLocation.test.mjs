import assert from "node:assert/strict";
import test from "node:test";
import { loadedFileLocation, withArtifactEntries } from "../src/fileLocation.ts";

test("filesystem-action requests preserve the preview source and discard unrelated session/ref", () => {
  const cases = [
    [{ source: "checkout", file: { path: "result.md", root: "clone" } }, { source: "repo", path: "result.md", sessionId: undefined, ref: undefined }],
    [{ source: "checkout", file: { path: "result.md", root: "branch" } }, { source: "repo", path: "result.md", sessionId: undefined, ref: "experiment/main" }],
    [{ source: "artifact", file: { path: "result.md" } }, { source: "artifacts", path: "result.md" }],
    [{ source: "absolute", file: { path: "/tmp/report.md" } }, { source: "abs", path: "/tmp/report.md" }],
  ];
  for (const [loaded, expected] of cases) {
    assert.deepEqual(loadedFileLocation(loaded, "chat", "experiment/main"), expected, `${loaded.source}/${loaded.file.root ?? ""}`);
  }
});

const entry = path => ({ path, name: path.split("/").at(-1), isDir: false, size: 12, modifiedAt: 10 });
const directory = (path, children) => ({ ...entry(path), isDir: true, children });
test("search hits gain missing ancestors and merge into the tree without duplicates or mutation", () => {
  const initial = [directory("reports", [entry("reports/old.md")]), entry("first.md")];
  const before = structuredClone(initial);
  const result = withArtifactEntries(initial, [entry("extra/deep/final.md"), entry("reports/new.md"), entry("reports/old.md")]);
  assert.deepEqual(result, [
    { path: "extra", name: "extra", isDir: true, size: 0, modifiedAt: 0, children: [
      { path: "extra/deep", name: "deep", isDir: true, size: 0, modifiedAt: 0, children: [entry("extra/deep/final.md")] },
    ] },
    directory("reports", [entry("reports/new.md"), { ...entry("reports/old.md"), children: undefined }]),
    { ...entry("first.md"), children: undefined },
  ]);
  assert.deepEqual(initial, before);
});

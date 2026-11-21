import assert from "node:assert/strict";
import test from "node:test";
import { artifactFiles, looksLikeFilePath, resolveArtifactReference, resolveFileReference } from "../src/fileReferences.ts";

const entries = ["README.md", "src/train.py", "src/data/loader.py", "scripts/train.py", "ui/src/[id].tsx", "configs/base.yaml"];

test("only path-shaped code reads as a file reference", () => {
  for (const text of ["src/train.py", "train.py:42", "src/**/*.py", "scripts/train.py#L3-L9"]) {
    assert.equal(looksLikeFilePath(text), true, text);
  }
  for (const text of ["configs/", "/etc/hosts", "../x.py", "v1", "e.g.", "two words.py", "https://a.b/c.py", "torch"]) {
    assert.equal(looksLikeFilePath(text), false, text);
  }
});

test("an exact repo path is certain and keeps its line", () => {
  assert.deepEqual(resolveFileReference(entries, "src/train.py:42"), { kind: "exact", path: "src/train.py", line: 42 });
  assert.deepEqual(resolveFileReference(entries, "./README.md"), { kind: "exact", path: "README.md", line: undefined });
});

test("a bare name or trailing directories is a choice, never a guess", () => {
  assert.deepEqual(resolveFileReference(entries, "train.py").paths, ["scripts/train.py", "src/train.py"]);
  assert.deepEqual(resolveFileReference(entries, "data/loader.py").paths, ["src/data/loader.py"]);
  assert.equal(resolveFileReference(entries, "data/loader.py").kind, "choices");
  assert.equal(resolveFileReference(entries, "SRC/Train.py").kind, "choices");
});

test("globs list every match, anchored by their directory", () => {
  assert.deepEqual(resolveFileReference(entries, "src/**/*.py").paths, ["src/train.py", "src/data/loader.py"]);
  assert.deepEqual(resolveFileReference(entries, "*.yaml").paths, ["configs/base.yaml"]);
  assert.deepEqual(resolveFileReference(entries, "src/*.{py,md}").paths, ["src/train.py"]);
  assert.equal(resolveFileReference(entries, "src/*.rs").kind, "none");
});

test("a long match list is capped but reports its full size", () => {
  const many = Array.from({ length: 450 }, (_, i) => `gen/part${i}.py`);
  const resolved = resolveFileReference(many, "gen/*.py");
  assert.equal(resolved.paths.length, 200);
  assert.equal(resolved.total, 450);
});

test("text that names no file resolves to nothing", () => {
  assert.equal(resolveFileReference(entries, "torch.nn").kind, "none");
  assert.equal(resolveFileReference(entries, "missing/file.py").kind, "none");
});

test("bracketed names match literally", () => {
  assert.equal(resolveFileReference(entries, "ui/src/[id].tsx").kind, "exact");
});

test("an artifact cited with or without its prefix resolves in the artifacts directory", () => {
  const files = artifactFiles([
    { name: "report.md", path: "report.md", isDir: false, size: 1, modifiedAt: 0 },
    { name: "exp1", path: "exp1", isDir: true, size: 0, modifiedAt: 0, children: [
      { name: "results_table.md", path: "exp1/results_table.md", isDir: false, size: 1, modifiedAt: 0 },
    ] },
  ]);
  assert.deepEqual(files, ["report.md", "exp1/results_table.md"]);
  assert.deepEqual(resolveArtifactReference(files, "report.md"), { kind: "exact", path: "artifacts/report.md", line: undefined });
  assert.deepEqual(resolveArtifactReference(files, "artifacts/report.md:4"), { kind: "exact", path: "artifacts/report.md", line: 4 });
  assert.deepEqual(resolveArtifactReference(files, "results_table.md").paths, ["artifacts/exp1/results_table.md"]);
  assert.deepEqual(resolveArtifactReference(files, "artifacts/results_table.md").paths, ["artifacts/exp1/results_table.md"]);
  assert.equal(resolveArtifactReference(files, "artifacts/missing.md").kind, "none");
  assert.equal(resolveArtifactReference(files, "./Artifacts/report.md").path, "artifacts/report.md");
});

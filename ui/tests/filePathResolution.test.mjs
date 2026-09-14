import assert from "node:assert/strict";
import test from "node:test";

import { escapeRegExp, parseFilePath, fileBranchLabel } from "../src/filePathResolution.ts";

test("escapeRegExp escapes every regex metacharacter", () => {
  const raw = "a.b*c+d?e^f$g{h}i(j)k|l[m]n\\o";
  const pattern = new RegExp(`^${escapeRegExp(raw)}$`);
  assert.ok(pattern.test(raw));
});

test("an artifacts/-prefixed path strips the prefix and is tagged artifacts", () => {
  assert.deepEqual(parseFilePath("artifacts/figure.svg"), { path: "figure.svg", source: "artifacts" });
});

test("artifacts/ with nothing after it resolves to nothing", () => {
  assert.equal(parseFilePath("artifacts/"), null);
});

test("a home-anchored path is disk, never a repo file", () => {
  assert.deepEqual(parseFilePath("~"), { path: "~", source: "abs" });
  assert.deepEqual(parseFilePath("~/notes.md"), { path: "~/notes.md", source: "abs" });
});

test("a relative path inherits the click context's session and has no source tag", () => {
  const tab = parseFilePath("src/main.rs", "/repo/clone", "session-1");
  assert.equal(tab?.path, "src/main.rs");
  assert.equal(tab?.sessionId, "session-1");
  assert.equal(tab?.source, undefined);
});

test("an absolute path under the repo clone strips to repo-relative with no session", () => {
  const tab = parseFilePath("/repo/clone/src/main.rs", "/repo/clone", "session-1");
  assert.equal(tab?.path, "src/main.rs");
  assert.equal(tab?.sessionId, undefined);
  assert.equal(tab?.source, undefined);
});

test("an absolute path exactly at the repo clone root resolves to an empty relative path, which is unusable", () => {
  assert.equal(parseFilePath("/repo/clone", "/repo/clone"), null);
});

test("an absolute path under the artifacts dir is tagged artifacts, stripped to relative", () => {
  const tab = parseFilePath("/data/proj/files/paper.tex", undefined, undefined, "/data/proj/files");
  assert.deepEqual(tab, { path: "paper.tex", source: "artifacts" });
});

test("macOS's /private symlink prefix is stripped from both the reported path and the stored dir", () => {
  const viaPrivate = parseFilePath("/private/tmp/clone/src/main.rs", "/tmp/clone");
  assert.equal(viaPrivate?.path, "src/main.rs");
  const viaBoth = parseFilePath("/tmp/clone/src/main.rs", "/private/tmp/clone");
  assert.equal(viaBoth?.path, "src/main.rs");
});

test("a worktree-layout path falls back to extracting the session id and relative path", () => {
  const tab = parseFilePath("/home/user/.local/share/openresearch/worktrees/proj-1/session-9/src/lib.rs");
  assert.deepEqual(tab, { path: "src/lib.rs", sessionId: "session-9" });
});

test("a legacy repos-layout path falls back to a bare repo-relative path", () => {
  const tab = parseFilePath("/home/user/.local/share/openresearch/repos/owner/repo/src/lib.rs");
  assert.equal(tab?.path, "src/lib.rs");
  assert.equal(tab?.sessionId, undefined);
});

test("the files/<slug>/ fallback requires the exact slug when one is known", () => {
  const path = "/some/other/root/files/my-proj/notes.md";
  assert.deepEqual(parseFilePath(path, undefined, undefined, undefined, "my-proj"), {
    path: "notes.md",
    source: "artifacts",
  });
  // A mismatched slug matches none of the fallbacks, so the path falls through
  // to the disk-read case unchanged.
  assert.deepEqual(parseFilePath(path, undefined, undefined, undefined, "other-proj"), {
    path,
    source: "abs",
  });
});

test("a slug containing regex metacharacters is matched literally, not as a pattern", () => {
  const path = "/root/files/a.b+c/notes.md";
  assert.deepEqual(parseFilePath(path, undefined, undefined, undefined, "a.b+c"), {
    path: "notes.md",
    source: "artifacts",
  });
});

test("an unrecognized absolute path reads straight off disk", () => {
  assert.deepEqual(parseFilePath("/Users/me/.ssh/config"), { path: "/Users/me/.ssh/config", source: "abs" });
});

test("fileBranchLabel: artifacts and abs files never carry a branch", () => {
  assert.equal(fileBranchLabel({ path: "x", source: "artifacts" }, "main"), undefined);
  assert.equal(fileBranchLabel({ path: "x", source: "abs" }, "main"), undefined);
});

test("fileBranchLabel: an explicit ref wins over the branch label and baseline", () => {
  assert.equal(fileBranchLabel({ path: "x", ref: "feature", branchLabel: "other" }, "main"), "feature");
});

test("fileBranchLabel: a branch label is used when there is no ref", () => {
  assert.equal(fileBranchLabel({ path: "x", branchLabel: "feature" }, "main"), "feature");
});

test("fileBranchLabel: falls back to the baseline branch when neither ref nor label is set", () => {
  assert.equal(fileBranchLabel({ path: "x" }, "main"), "main");
  assert.equal(fileBranchLabel({ path: "x" }), undefined);
});

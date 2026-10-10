import assert from "node:assert/strict";
import test from "node:test";
import {
  fileMentionContext,
  insertFileMention,
  matchFileMentions,
  mentionCandidates,
  splitMentionPath,
} from "../src/fileMentions.ts";

const pool = mentionCandidates(["README.md", "src/main.rs", "src/store/mod.rs", "ui/src/main.tsx"]);

test("a mention opens only where @ starts the token", () => {
  assert.deepEqual(fileMentionContext("see @src/ma", 11), { query: "src/ma", start: 4, end: 11 });
  assert.equal(fileMentionContext("mail me@example.com", 19), null);
  assert.equal(fileMentionContext("no mention", 10), null);
});

test("an empty or directory query lists one level, directories first", () => {
  assert.deepEqual(matchFileMentions(pool, "").map((m) => m.path), ["src/", "ui/", "README.md"]);
  assert.deepEqual(matchFileMentions(pool, "src/").map((m) => m.path), ["src/store/", "src/main.rs"]);
});

test("name-prefix matches rank ahead of path matches", () => {
  assert.deepEqual(matchFileMentions(pool, "main").map((m) => m.path), ["src/main.rs", "ui/src/main.tsx"]);
  assert.deepEqual(matchFileMentions(pool, "store").map((m) => m.path), ["src/store/", "src/store/mod.rs"]);
});

test("picking a file ends the token; picking a directory keeps it open", () => {
  const context = fileMentionContext("read @ma please", 8);
  assert.deepEqual(insertFileMention("read @ma please", context, { path: "src/main.rs", directory: false }), {
    text: "read @src/main.rs please",
    cursor: 18,
  });
  assert.deepEqual(insertFileMention("read @sr", fileMentionContext("read @sr", 8), { path: "src/", directory: true }), {
    text: "read @src/",
    cursor: 10,
  });
  assert.deepEqual(insertFileMention("@ma", fileMentionContext("@ma", 3), { path: "a.md", directory: false }), {
    text: "@a.md ",
    cursor: 6,
  });
});

test("matching ignores case but inserts the path as listed", () => {
  assert.deepEqual(matchFileMentions(pool, "readme").map((m) => m.path), ["README.md"]);
});

test("a path with whitespace is quoted, and an existing space is reused", () => {
  const context = fileMentionContext("see @my", 7);
  assert.deepEqual(insertFileMention("see @my", context, { path: "docs/my notes.md", directory: false }), {
    text: 'see @"docs/my notes.md" ',
    cursor: 24,
  });
  assert.deepEqual(insertFileMention("@ma\nnext", fileMentionContext("@ma\nnext", 3), { path: "a.md", directory: false }), {
    text: "@a.md\nnext",
    cursor: 5,
  });
});

test("a quoted directory ends the mention like a file", () => {
  assert.deepEqual(insertFileMention("@my", fileMentionContext("@my", 3), { path: "my dir/", directory: true }), {
    text: '@"my dir/" ',
    cursor: 11,
  });
});

test("menu rows split a path into name and parent, keeping a directory's slash", () => {
  assert.deepEqual(splitMentionPath("README.md"), { name: "README.md", parent: "" });
  assert.deepEqual(splitMentionPath("src/store/"), { name: "store/", parent: "src" });
  assert.deepEqual(splitMentionPath("a/b/c.rs"), { name: "c.rs", parent: "a/b" });
});

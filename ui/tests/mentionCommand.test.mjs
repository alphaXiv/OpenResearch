import assert from "node:assert/strict";
import test from "node:test";
import {
  insertMention,
  mentionBasename,
  mentionContext,
  rankMentionMatches,
  splitMentionTokens,
} from "../src/mentionCommand.ts";

test("mention context follows the caret anywhere in the message", () => {
  assert.deepEqual(mentionContext("@src/f", 6), { query: "src/f", start: 0, end: 6 });
  assert.deepEqual(mentionContext("look at @src/f now", 14), {
    query: "src/f",
    start: 8,
    end: 14,
  });
  assert.deepEqual(mentionContext("look at @ now", 9), { query: "", start: 8, end: 9 });
  assert.equal(mentionContext("email user@example.com", 22), null);
  // Where onChange looks once the space that finished a mention lands.
  assert.deepEqual(mentionContext("look at @src/foo.py now", 19), {
    query: "src/foo.py",
    start: 8,
    end: 19,
  });
});

test("session and message prefixes are not this menu's to open", () => {
  assert.equal(mentionContext("@session:chat_1", 16), null);
  assert.equal(mentionContext("@message:chat_1", 16), null);
  assert.equal(mentionContext("@SESSION:chat_1", 16), null);
});

test("picking a file replaces the token in place", () => {
  const text = "look at @src/f now";
  assert.deepEqual(insertMention(text, mentionContext(text, 13), "src/foo.py"), {
    text: "look at @src/foo.py now",
    cursor: 20,
  });
  const tail = "look at @src/f";
  assert.deepEqual(insertMention(tail, mentionContext(tail, 14), "src/foo.py"), {
    text: "look at @src/foo.py ",
    cursor: 20,
  });
});

test("known @path tokens split out wherever they were typed", () => {
  assert.deepEqual(splitMentionTokens("look at @src/foo.py please"), [
    { text: "look at ", mention: false },
    { text: "@src/foo.py", mention: true },
    { text: " please", mention: false },
  ]);
  assert.deepEqual(splitMentionTokens("@src/foo.py"), [{ text: "@src/foo.py", mention: true }]);
  assert.deepEqual(splitMentionTokens("no mentions here"), [
    { text: "no mentions here", mention: false },
  ]);
  // An email mid-word is never chipped — only a run starting with "@" is.
  assert.deepEqual(splitMentionTokens("email user@example.com about it"), [
    { text: "email user@example.com about it", mention: false },
  ]);
});

test("trailing sentence punctuation stays out of the chip", () => {
  assert.deepEqual(splitMentionTokens("see @src/foo.rs. it's broken"), [
    { text: "see ", mention: false },
    { text: "@src/foo.rs", mention: true },
    { text: ". it's broken", mention: false },
  ]);
});

test("splitting a message loses nothing — chips are painted by offset", () => {
  for (const text of [
    "use @src/foo.py please",
    "@src/foo.py",
    "  @src/foo.py  two  spaces  ",
    "line one\n@src/foo.py args\n\nline three",
    "@a.py@b.py @a.py\t@b.py",
  ])
    assert.equal(
      splitMentionTokens(text)
        .map((segment) => segment.text)
        .join(""),
      text,
    );
});

test("ranking prefers a basename match over a path-only match", () => {
  const entries = ["src/deep/nested/foo.py", "foo/other.py"];
  assert.deepEqual(rankMentionMatches(entries, "foo"), [
    "src/deep/nested/foo.py",
    "foo/other.py",
  ]);
});

test("ranking is fuzzy and caps at the given limit", () => {
  assert.deepEqual(rankMentionMatches(["src/foo_bar.py", "src/unrelated.py"], "fb"), [
    "src/foo_bar.py",
  ]);
  const many = Array.from({ length: 80 }, (_, i) => `src/file${i}.py`);
  assert.equal(rankMentionMatches(many, "file", 50).length, 50);
});

test("an empty query returns the entries as given, up to the limit", () => {
  assert.deepEqual(rankMentionMatches(["a.py", "b.py", "c.py"], "", 2), ["a.py", "b.py"]);
  assert.deepEqual(rankMentionMatches(["a.py", "b.py"], "   "), ["a.py", "b.py"]);
});

test("basenames fall back to the whole path when there is no slash", () => {
  assert.equal(mentionBasename("foo.py"), "foo.py");
  assert.equal(mentionBasename("src/deep/foo.py"), "foo.py");
  assert.equal(mentionBasename("session:chat_1"), "session:chat_1");
});

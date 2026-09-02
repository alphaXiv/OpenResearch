import assert from "node:assert/strict";
import test from "node:test";
import { activePath, forkPositions } from "../src/transcriptTree.ts";

/** u1 → a1, with a re-sampled a2 beside it; the conversation carries on under a1. */
const forked = [
  { id: "u1", role: "user", parentId: null },
  { id: "a1", role: "assistant", parentId: "u1" },
  { id: "a2", role: "assistant", parentId: "u1" },
  { id: "u2", role: "user", parentId: "a1" },
  { id: "a3", role: "assistant", parentId: "u2" },
];

const ids = (list) => list.map((m) => m.id);
const never = () => false;
const at = (list, id) => list.find((m) => m.id === id);

test("the active path walks parents from the leaf", () => {
  assert.deepEqual(ids(activePath(forked, "a3")), ["u1", "a1", "u2", "a3"]);
  // Switching forks hides everything that only exists under the other one.
  assert.deepEqual(ids(activePath(forked, "a2")), ["u1", "a2"]);
});

test("an absent or unknown leaf keeps the whole transcript", () => {
  // Identity is preserved, which memoized consumers rely on.
  assert.equal(activePath(forked, null), forked);
  assert.equal(activePath(forked, undefined), forked);
  // A pointer at a message that is gone must not blank the transcript.
  assert.equal(activePath(forked, "missing"), forked);
});

test("a message with no parentId field is treated as a root", () => {
  const legacy = [
    { id: "m1", role: "user" },
    { id: "m2", role: "assistant", parentId: "m1" },
  ];
  assert.deepEqual(ids(activePath(legacy, "m2")), ["m1", "m2"]);
});

test("the pager reports which fork is on screen and where it can go", () => {
  const path = activePath(forked, "a2");
  const positions = forkPositions(forked, path, [at(forked, "a2")], never);
  assert.deepEqual(positions.get("a2"), {
    count: 2,
    index: 1,
    prevId: "a1",
    nextId: undefined,
  });

  const other = activePath(forked, "a3");
  const first = forkPositions(forked, other, [at(forked, "a1")], never);
  assert.deepEqual(first.get("a1"), { count: 2, index: 0, prevId: undefined, nextId: "a2" });
});

test("a turn that was never re-sampled has a single fork", () => {
  const path = activePath(forked, "a3");
  const positions = forkPositions(forked, path, [at(forked, "u2"), at(forked, "a3")], never);
  assert.equal(positions.get("u2").count, 1);
  assert.equal(positions.get("a3").count, 1);
});

test("forks are counted where the replies diverge, not at the reply's tail", () => {
  // A reply that opens with a permission card, then answers: u1 → ap → am.
  const carded = [
    { id: "u1", role: "user", parentId: null },
    { id: "ap", role: "assistant", parentId: "u1" },
    { id: "am", role: "assistant", parentId: "ap" },
    { id: "ap2", role: "assistant", parentId: "u1" },
  ];
  const path = activePath(carded, "am");
  // Helper-level: an assistant bearer anchors to its user turn, not to its own tail.
  assert.deepEqual(forkPositions(carded, path, [at(carded, "am")], never).get("am"), {
    count: 2,
    index: 0,
    prevId: undefined,
    nextId: "ap2",
  });
});

test("a user turn pages over edited prompts only, never a stray reply", () => {
  const edited = [
    { id: "u1", role: "user", parentId: null },
    { id: "u1b", role: "user", parentId: null },
    // An interrupt marker left at the root must not inflate the prompt's pager.
    { id: "orphan", role: "assistant", parentId: null },
    { id: "a1", role: "assistant", parentId: "u1" },
  ];
  const path = activePath(edited, "a1");
  assert.deepEqual(forkPositions(edited, path, [at(edited, "u1")], never).get("u1"), {
    count: 2,
    index: 0,
    prevId: undefined,
    nextId: "u1b",
  });
});

test("a fork in the middle of three can page both ways", () => {
  const three = [
    { id: "u1", role: "user", parentId: null },
    { id: "a1", role: "assistant", parentId: "u1" },
    { id: "a2", role: "assistant", parentId: "u1" },
    { id: "a3", role: "assistant", parentId: "u1" },
  ];
  const path = activePath(three, "a2");
  assert.deepEqual(forkPositions(three, path, [at(three, "a2")], never).get("a2"), {
    count: 3,
    index: 1,
    prevId: "a1",
    nextId: "a3",
  });
});

test("a user message under a user anchor does not inflate a reply's pager", () => {
  // A turn that recorded no reply leaves the next prompt parented to a prompt.
  const mixed = [
    { id: "u1", role: "user", parentId: null },
    { id: "u2", role: "user", parentId: "u1" },
    { id: "a1", role: "assistant", parentId: "u1" },
  ];
  const path = activePath(mixed, "a1");
  assert.equal(forkPositions(mixed, path, [at(mixed, "a1")], never).get("a1").count, 1);
});

test("an optimistic bubble is not counted as a fork of the turn it sits beside", () => {
  const sending = [
    { id: "u1", role: "user", parentId: null },
    { id: "a1", role: "assistant", parentId: "u1" },
    { id: "local-123", role: "user", parentId: "a1" },
    { id: "u2", role: "user", parentId: "a1" },
  ];
  const path = activePath(sending, "u2");
  const isLocal = (id) => id.startsWith("local-");
  assert.equal(forkPositions(sending, path, [at(sending, "u2")], isLocal).get("u2").count, 1);
});

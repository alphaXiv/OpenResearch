import assert from "node:assert/strict";
import test from "node:test";
import { delegateAncestors } from "../src/delegates.ts";

const chat = (id, parentId) => ({ id, delegate: parentId ? { parentId } : null });

test("a delegate's ancestors run from the root chat to its parent", () => {
  const sessions = [chat("root"), chat("child", "root"), chat("grandchild", "child")];
  assert.deepEqual(delegateAncestors(sessions, sessions[2]).map((session) => session.id), ["root", "child"]);
});

test("a chat nobody delegated to has no ancestors", () => {
  const sessions = [chat("root")];
  assert.deepEqual(delegateAncestors(sessions, sessions[0]), []);
  assert.deepEqual(delegateAncestors(sessions, undefined), []);
});

test("the trail stops below a parent missing from the list", () => {
  const sessions = [chat("child", "gone"), chat("grandchild", "child")];
  assert.deepEqual(delegateAncestors(sessions, sessions[1]).map((session) => session.id), ["child"]);
});

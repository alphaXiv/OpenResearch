import assert from "node:assert/strict";
import test from "node:test";
import { applyNavOrder, moveNavItem } from "../src/navOrder.ts";

const ids = ["general", "agents", "chat", "integrations", "storage"];

test("a saved order wins, and an empty one keeps the default", () => {
  assert.deepEqual(applyNavOrder(ids, []), ids);
  assert.deepEqual(
    applyNavOrder(ids, ["storage", "chat", "general", "agents", "integrations"]),
    ["storage", "chat", "general", "agents", "integrations"],
  );
});

test("items added since the order was saved keep their place, removed ones drop out", () => {
  assert.deepEqual(
    applyNavOrder(ids, ["agents", "general", "storage", "gone"]),
    ["agents", "chat", "integrations", "general", "storage"],
  );
  assert.deepEqual(applyNavOrder(["a", "b"], ["b"]), ["a", "b"]);
});

test("dragging an item onto another takes its place", () => {
  assert.deepEqual(moveNavItem(ids, "storage", "general"), ["storage", "general", "agents", "chat", "integrations"]);
  assert.deepEqual(moveNavItem(ids, "general", "chat"), ["agents", "chat", "general", "integrations", "storage"]);
  assert.deepEqual(moveNavItem(ids, "chat", "chat"), ids);
  assert.deepEqual(moveNavItem(ids, "missing", "chat"), ids);
});

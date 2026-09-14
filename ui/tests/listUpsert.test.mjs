import assert from "node:assert/strict";
import test from "node:test";

import { upsertById } from "../src/listUpsert.ts";

test("upsertById appends an item whose id is not yet in the list", () => {
  assert.deepEqual(upsertById([{ id: "a" }], { id: "b" }), [{ id: "a" }, { id: "b" }]);
});

test("upsertById replaces the item sharing the same id in place", () => {
  const list = [{ id: "a", n: 1 }, { id: "b", n: 2 }, { id: "c", n: 3 }];
  assert.deepEqual(upsertById(list, { id: "b", n: 20 }), [{ id: "a", n: 1 }, { id: "b", n: 20 }, { id: "c", n: 3 }]);
});

test("upsertById does not mutate the input list", () => {
  const list = [{ id: "a", n: 1 }];
  const next = upsertById(list, { id: "a", n: 2 });
  assert.deepEqual(list, [{ id: "a", n: 1 }]);
  assert.notEqual(next, list);
});

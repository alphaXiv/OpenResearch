import assert from "node:assert/strict";
import test from "node:test";
import { STALE_BRANCH_MS, autoCollapsedIds, effectiveCollapsedIds, toggleCollapsed } from "../src/treeCollapse.ts";

const node = (id, children = []) => ({ id, children });
const now = 10 * STALE_BRANCH_MS;

test("auto tidy folds stale and failed side branches, never roots, live work or the newest branch", () => {
  const roots = [node("root", [
    node("stale", [node("stale-child")]),
    node("failed", [node("failed-child")]),
    node("live", [node("live-child")]),
    node("newest", [node("newest-child")]),
    node("leaf"),
  ])];
  const activity = new Map([
    ["root", { live: false, failed: false, lastActivity: 0 }],
    ["stale", { live: false, failed: false, lastActivity: now - 2 * STALE_BRANCH_MS }],
    ["stale-child", { live: false, failed: false, lastActivity: now - 2 * STALE_BRANCH_MS }],
    ["failed", { live: false, failed: true, lastActivity: now - 10 }],
    ["failed-child", { live: false, failed: false, lastActivity: now - 10 }],
    ["live", { live: false, failed: false, lastActivity: now - 3 * STALE_BRANCH_MS }],
    ["live-child", { live: true, failed: false, lastActivity: now - 3 * STALE_BRANCH_MS }],
    ["newest", { live: false, failed: true, lastActivity: now - 3 * STALE_BRANCH_MS }],
    ["newest-child", { live: false, failed: false, lastActivity: now }],
    ["leaf", { live: false, failed: true, lastActivity: 0 }],
  ]);
  const folded = autoCollapsedIds(roots, (id) => activity.get(id), now);
  assert.deepEqual([...folded].sort(), ["failed", "stale"]);
});

test("manual choices override auto tidy and revealed paths stay open", () => {
  const result = effectiveCollapsedIds({
    manualCollapsed: ["a", "b"],
    manualExpanded: ["c"],
    auto: new Set(["c", "d"]),
    reveal: new Set(["b"]),
  });
  assert.deepEqual([...result].sort(), ["a", "d"]);
  assert.deepEqual(toggleCollapsed({ collapsed: ["a"], expanded: [] }, "a", true), { collapsed: [], expanded: ["a"] });
  assert.deepEqual(toggleCollapsed({ collapsed: [], expanded: ["a"] }, "a", false), { collapsed: ["a"], expanded: [] });
});

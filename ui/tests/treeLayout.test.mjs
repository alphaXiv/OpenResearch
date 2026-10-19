import assert from "node:assert/strict";
import test from "node:test";
import { compactTreeLayout } from "../src/treeLayout.ts";

const node = (id, children = [], width = 264, height = 180) => ({ id, children, width, height });
function rectangles(roots, positions) {
  return roots.flatMap((item) => [{ ...item, ...positions.get(item.id) }, ...rectangles(item.children, positions)]);
}
function assertSeparated(items) {
  for (let i = 0; i < items.length; i++) {
    for (let j = i + 1; j < items.length; j++) {
      const a = items[i], b = items[j];
      assert.ok(a.x + a.width <= b.x || b.x + b.width <= a.x || a.y + a.height <= b.y || b.y + b.height <= a.y, `${a.id} overlaps ${b.id}`);
    }
  }
}

test("mixed card sizes and multiple roots do not overlap", () => {
  const roots = [node("baseline", [node("tall", [node("grandchild")], 264, 310), node("pill", [], 148, 44), node("trial")]), node("other", [node("other-child", [], 264, 280)])];
  const positions = compactTreeLayout(roots);
  const items = rectangles(roots, positions);
  assert.equal(positions.size, 7);
  assertSeparated(items);
  assert.ok(items.every((item) => item.x >= 0 && item.y >= 0));
  assert.ok(positions.get("grandchild").y >= positions.get("tall").y + 310 + 48);
  const center = (id, width) => positions.get(id).x + width / 2;
  assert.equal(center("baseline", 264), (center("tall", 264) + center("trial", 264)) / 2);
});

test("uneven branches use space between their contours", () => {
  const roots = [node("root", [node("left", [node("wide-left", [], 400, 40)], 100, 40), node("right", [node("middle", [node("wide-right", [], 400, 40)], 100, 40)], 100, 40)], 100, 40)];
  const positions = compactTreeLayout(roots);
  const items = rectangles(roots, positions);
  assertSeparated(items);
  assert.ok(Math.max(...items.map((item) => item.x + item.width)) < 750);
  assert.deepEqual(compactTreeLayout(roots), positions);
});

test("growing cards moves the next row down and compacts it again when shrinking", () => {
  const root = node("root", [node("child")]);
  const before = compactTreeLayout([root]);
  root.height = 400;
  const grown = compactTreeLayout([root]);
  assert.equal(grown.get("child").y, 448);
  root.height = 180;
  assert.deepEqual(compactTreeLayout([root]), before);
});

test("empty and single-node trees have stable layouts", () => {
  assert.deepEqual([...compactTreeLayout([])], []);
  assert.deepEqual([...compactTreeLayout([node("only")])], [["only", { x: 0, y: 0 }]]);
});

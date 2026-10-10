import assert from "node:assert/strict";
import test from "node:test";
import { buildRoutes, parseRouteFolds, routeRows, routeRun, visibleRouteId } from "../src/experimentRoutes.ts";

const exp = (id, parentExperimentId = null, extra = {}) => ({ id, parentExperimentId, createdAt: 1, chatSessionId: "A", archived: false, ...extra });
const ids = (routes, folds = new Set()) => routeRows(routes, folds).map((row) => row.node.experiment.id);

test("stable chronological sibling ordering, multiple roots and exact edges", () => {
  const input = [exp("sibling", "root", { createdAt: 3 }), exp("root"), exp("other"), exp("child", "root")];
  const routes = buildRoutes(input, null, false);
  assert.deepEqual(ids(routes), ["other", "root", "child", "sibling"]);
  assert.deepEqual(ids(buildRoutes([...input].reverse(), null, false)), ids(routes));
  assert.equal(routes.nodes.get("child").parentId, "root");
});

test("A → B → A retains the other task and archived ancestor without shortcuts", () => {
  const routes = buildRoutes([exp("A1", null, { archived: true }), exp("B", "A1", { chatSessionId: "B" }), exp("A2", "B"), exp("unrelated", null, { chatSessionId: "B" })], "A", false);
  assert.deepEqual(ids(routes), ["A1", "B", "A2"]);
  assert.equal(routes.nodes.get("A1").context, true);
  assert.equal(routes.nodes.get("B").context, true);
  assert.equal(routes.nodes.get("A2").context, false);
  assert.equal(routes.nodes.get("A2").parentId, "B");
});

test("missing parent and cycles are boundaries, never claimed as baselines", () => {
  const input = [exp("missing", "gone"), exp("self", "self"), exp("z", "a"), exp("a", "z")];
  const routes = buildRoutes(input, null, false);
  assert.equal(routes.nodes.get("missing").boundary, "missing");
  assert.equal(routes.nodes.get("self").boundary, "cycle");
  assert.equal(routes.nodes.get("a").boundary, "cycle");
  assert.equal(routes.nodes.get("z").parentId, "a");
  assert.equal(new Set(ids(routes)).size, input.length);
  assert.deepEqual(ids(buildRoutes([...input].reverse(), null, false)), ids(routes));
});

test("archive and task filters hide unrelated leaves and handle empty scopes", () => {
  const input = [exp("root"), exp("hidden", "root", { archived: true }), exp("other", null, { chatSessionId: "B" })];
  assert.deepEqual(ids(buildRoutes(input, "A", false)), ["root"]);
  assert.deepEqual(ids(buildRoutes(input, "A", true)), ["root", "hidden"]);
  assert.deepEqual(ids(buildRoutes(input, "absent", false)), []);
  assert.deepEqual(ids(buildRoutes([], null, false)), []);
});

test("folds survive new API objects; hidden focus/selection resolves to an ancestor", () => {
  const input = [exp("root"), exp("child", "root"), exp("grandchild", "child")];
  const folds = parseRouteFolds(["child", 7, null]);
  const routes = buildRoutes(input, null, false);
  const rows = routeRows(routes, folds);
  assert.deepEqual(ids(routes, folds), ["root", "child"]);
  assert.equal(visibleRouteId("grandchild", routes, rows), "child");
  assert.equal(visibleRouteId("deleted", routes, rows), "root");
  assert.deepEqual(ids(buildRoutes(input.map((node) => ({ ...node })), null, false), folds), ["root", "child"]);
  assert.deepEqual([...parseRouteFolds({ malformed: true })], []);
});

test("live/cancelling run wins over a newer terminal attempt; ties are stable", () => {
  const runs = [{ id: "done", status: "done", createdAt: 3 }, { id: "live", status: "running", cancelRequested: true, createdAt: 1 }];
  assert.equal(routeRun(runs).id, "live");
  assert.equal(routeRun([{ id: "z", status: "failed", createdAt: 1 }, { id: "a", status: "done", createdAt: 1 }]).id, "a");
  assert.equal(routeRun([]), null);
  assert.equal(runs[0].id, "done");
});

test("deep routes use iterative traversal and wide sibling ARIA positions are correct", () => {
  const deep = Array.from({ length: 1000 }, (_, i) => exp(String(i), i ? String(i - 1) : null, { createdAt: i }));
  const rows = routeRows(buildRoutes(deep, null, false), new Set());
  assert.equal(rows.length, 1000);
  assert.equal(rows.at(-1).depth, 999);
  const wide = [exp("root"), ...Array.from({ length: 170 }, (_, i) => exp(`child-${i}`, "root", { createdAt: i + 2 }))];
  const children = routeRows(buildRoutes(wide, null, false), new Set()).slice(1);
  assert.equal(children.at(-1).position, 170);
  assert.equal(children[0].size, 170);
});

test("connector continuation reflects siblings at each ancestor level", () => {
  const routes = buildRoutes([exp("root"), exp("a", "root"), exp("b", "root"), exp("a1", "a"), exp("a2", "a"), exp("deep", "a1")], null, false);
  const rows = routeRows(routes, new Set());
  assert.deepEqual(rows.find((row) => row.node.experiment.id === "a1").continuation, [true, false]);
  assert.deepEqual(rows.find((row) => row.node.experiment.id === "deep").continuation, [true, true, false]);
});

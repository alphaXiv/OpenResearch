import assert from "node:assert/strict";
import { test } from "node:test";
import { fitSidebarRows, sidebarRowHeight } from "../src/sidebarLayout.ts";

test("sidebar fits complete rows and keeps an older selected chat visible", () => {
  const first = { id: "first" };
  const second = { id: "second" };
  const rows = [
    { kind: "project", project: first },
    { kind: "chat", project: first, session: { id: "one" } },
    { kind: "chat", project: first, session: { id: "two" } },
    { kind: "project", project: second },
    { kind: "chat", project: second, session: { id: "selected" } },
  ];
  assert.deepEqual(fitSidebarRows(rows, 140, null), rows.slice(0, 3));
  const selected = fitSidebarRows(rows, 140, "selected");
  assert.ok(selected.includes(rows[4]));
  assert.ok(selected.reduce((height, row) => height + sidebarRowHeight(row), 0) <= 140);
});

test("sidebar keeps a trailing collapsed project header", () => {
  const first = { id: "first" };
  const second = { id: "second" };
  const rows = [
    { kind: "project", project: first, collapsed: false, busy: false },
    { kind: "chat", project: first, session: { id: "one" } },
    { kind: "project", project: second, collapsed: true, busy: false },
  ];
  assert.deepEqual(fitSidebarRows(rows, 106, null), rows);
  assert.deepEqual(fitSidebarRows([...rows.slice(0, 2), { ...rows[2], collapsed: false }], 106, null), rows.slice(0, 2));
});

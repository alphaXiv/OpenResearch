import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import * as React from "react";
import ts from "typescript";

const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/components/ProjectsHome.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2022 },
}).outputText;
const projects = Object.freeze([
  Object.freeze({ id: "a", name: "Alpha", createdAt: 30 }),
  Object.freeze({ id: "b", name: "Beta", createdAt: 20 }),
  Object.freeze({ id: "c", name: "Gamma", createdAt: 10 }),
]);

function memoryStorage() {
  const values = new Map();
  const writes = [];
  return {
    values, writes,
    getItem: (key) => values.get(key) ?? null,
    setItem(key, value) { values.set(key, value); writes.push([key, value]); },
  };
}

function elements(node) {
  if (Array.isArray(node)) return node.flatMap(elements);
  return React.isValidElement(node) ? [node, ...elements(node.props.children)] : [];
}

const hasClass = (node, name) => node.props.className?.split(" ").includes(name);
function event(extra = {}) {
  return {
    defaultPrevented: false, propagationStopped: false,
    preventDefault() { this.defaultPrevented = true; },
    stopPropagation() { this.propagationStopped = true; },
    ...extra,
  };
}

// Like the workspace hook tests, retain hook state between renders and invoke
// the component's real handlers. Ordering and persistence are never mocked.
function home({ storage = memoryStorage(), runtime = { kind: "local" }, activity = [], items = projects } = {}) {
  const state = [];
  const opened = [];
  let cursor = 0;
  let tree;
  const mocks = {
    react: {
      ...React,
      useState(initial) {
        const index = cursor++;
        if (!(index in state)) state[index] = typeof initial === "function" ? initial() : initial;
        return [state[index], (value) => { state[index] = typeof value === "function" ? value(state[index]) : value; }];
      },
    },
    "@tanstack/react-query": { useQuery: () => ({ data: activity }) },
    "../queries/projects": { listProjectActivityQuery: () => ({}) },
    "../RemoteRuntime": { useRuntime: () => runtime },
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
    "../i18n": { autoDir: (value) => value, ltr: (value) => value },
    "../api": { fmtNumber: String, timeAgo: () => "now" },
    "./BackendLogos": { GitHubMark: () => null },
    "./NewProjectForm": { NewProjectForm: () => null },
    "./ui": { Button: "button" },
    "lucide-react": { GripVertical: () => null, Plus: () => null, Trash2: () => null },
  };
  const exports = {};
  new Function("require", "exports", "localStorage", compiled)((name) => {
    if (name in mocks) return mocks[name];
    if (name === "react/jsx-runtime") return require(name);
    throw new Error(`Unexpected ProjectsHome dependency: ${name}`);
  }, exports, storage);
  function render(nextItems = items, nextActivity = activity) {
    items = nextItems;
    activity = nextActivity;
    cursor = 0;
    tree = exports.ProjectsHome({ projects: items, onOpen: (id) => opened.push(id) });
  }
  const rows = () => elements(tree).filter((node) => hasClass(node, "project-row"));
  const row = (id) => rows().find((node) => node.key === id);
  const handle = (id) => elements(row(id)).find((node) => node.type === "button" && node.props.draggable);
  render();
  return {
    storage, opened, render, row, handle,
    ids: () => rows().map((node) => node.key),
    key(id, key) {
      const e = event({ key });
      handle(id).props.onKeyDown(e);
      render();
      return e;
    },
    startDrag(id) {
      const data = new Map();
      const dataTransfer = { setData: (type, value) => data.set(type, value) };
      handle(id).props.onDragStart(event({ dataTransfer }));
      render();
      return { data, dataTransfer };
    },
    drop(id) {
      const e = event();
      row(id).props.onDrop(e);
      render();
      return e;
    },
  };
}

test("without a saved order projects use activity, creation time and name without mutating props", () => {
  const view = home({ activity: [{ projectId: "c", lastMessageAt: 40 }] });
  assert.deepEqual(view.ids(), ["c", "a", "b"]);
  view.render([
    { id: "b", name: "Beta", createdAt: 20 },
    { id: "a", name: "Alpha", createdAt: 20 },
  ], []);
  assert.deepEqual(view.ids(), ["a", "b"]);
  assert.deepEqual(projects.map((project) => project.id), ["a", "b", "c"]);
  assert.equal(view.storage.writes.length, 0);
});

test("arrow keys move in both directions, consume the event and persist the displayed order", () => {
  const view = home();
  const down = view.key("a", "ArrowDown");
  assert.deepEqual(view.ids(), ["b", "a", "c"]);
  assert.equal(down.defaultPrevented, true);
  assert.equal(down.propagationStopped, true);
  const up = view.key("c", "ArrowUp");
  assert.deepEqual(view.ids(), ["b", "c", "a"]);
  assert.equal(up.defaultPrevented, true);
  assert.equal(up.propagationStopped, true);
  assert.deepEqual(JSON.parse(view.storage.values.get("orx:project-order:local")), ["b", "c", "a"]);
  assert.deepEqual(view.opened, []);
  elements(view.row("b")).find((node) => hasClass(node, "project-row-open")).props.onClick();
  assert.deepEqual(view.opened, ["b"]);
});

test("boundary moves and unrelated keys neither reorder nor write storage", () => {
  const view = home();
  view.key("a", "ArrowUp");
  view.key("c", "ArrowDown");
  for (const key of ["Enter", " ", "ArrowLeft", "ArrowRight"]) {
    const e = view.key("b", key);
    assert.equal(e.defaultPrevented, false);
    assert.equal(e.propagationStopped, false);
  }
  assert.deepEqual(view.ids(), ["a", "b", "c"]);
  assert.equal(view.storage.writes.length, 0);
});

test("saved order survives remount and activity refreshes", () => {
  const view = home();
  view.key("c", "ArrowUp");
  view.key("c", "ArrowUp");
  const reloaded = home({ storage: view.storage });
  assert.deepEqual(reloaded.ids(), ["c", "a", "b"]);
  reloaded.render(projects, [{ projectId: "b", lastMessageAt: 100 }]);
  assert.deepEqual(reloaded.ids(), ["c", "a", "b"]);
});

test("new projects appear above saved projects, ordered by activity until explicitly moved", () => {
  const view = home();
  view.key("c", "ArrowUp");
  const items = [...projects, { id: "d", name: "Delta", createdAt: 1 }, { id: "e", name: "Epsilon", createdAt: 2 }];
  view.render(items);
  assert.deepEqual(view.ids(), ["e", "d", "a", "c", "b"]);
  view.key("e", "ArrowDown");
  assert.deepEqual(home({ storage: view.storage, items }).ids(), ["d", "e", "a", "c", "b"]);
});

test("deleted projects are ignored and the next move saves only current project IDs", () => {
  const view = home();
  view.key("c", "ArrowUp");
  view.render(projects.filter((project) => project.id !== "a"));
  assert.deepEqual(view.ids(), ["c", "b"]);
  view.key("b", "ArrowUp");
  assert.deepEqual(JSON.parse(view.storage.values.get("orx:project-order:local")), ["b", "c"]);
});

test("local and SSH hosts/databases keep separate orders while reconnecting restores the same workspace", () => {
  const storage = memoryStorage();
  const ssh = (host, database, id = "session-one") => ({ kind: "ssh", session: { id, host, installPaths: { database } } });
  const local = home({ storage });
  local.key("a", "ArrowDown");
  const remote = home({ storage, runtime: ssh("alice@host-one", "/db/one") });
  assert.deepEqual(remote.ids(), ["a", "b", "c"]);
  remote.key("c", "ArrowUp");
  assert.deepEqual(home({ storage, runtime: ssh("alice@host-one", "/db/two") }).ids(), ["a", "b", "c"]);
  assert.deepEqual(home({ storage, runtime: ssh("alice@host-two", "/db/one") }).ids(), ["a", "b", "c"]);
  assert.deepEqual(home({ storage, runtime: ssh("alice@host-one", "/db/one", "reconnected") }).ids(), ["a", "c", "b"]);
  assert.deepEqual(home({ storage }).ids(), ["b", "a", "c"]);
  assert.equal(storage.values.size, 2);
});

test("invalid saved data falls back to activity ordering and is repaired by a move", () => {
  for (const invalid of ["{", "null", "{}", '"a"', '["a", 1]']) {
    const storage = memoryStorage();
    storage.values.set("orx:project-order:local", invalid);
    const view = home({ storage });
    assert.deepEqual(view.ids(), ["a", "b", "c"]);
    view.key("b", "ArrowUp");
    assert.deepEqual(home({ storage }).ids(), ["b", "a", "c"]);
  }
});

test("storage read and write failures leave in-memory reordering usable", () => {
  const storage = {
    getItem() { throw new Error("Storage blocked"); },
    setItem() { throw new Error("Quota exceeded"); },
  };
  const view = home({ storage });
  assert.deepEqual(view.ids(), ["a", "b", "c"]);
  view.key("c", "ArrowUp");
  view.key("c", "ArrowUp");
  assert.deepEqual(view.ids(), ["c", "a", "b"]);
});

test("dragging down and up saves the target position and clears drag feedback", () => {
  const view = home();
  const { data, dataTransfer } = view.startDrag("a");
  assert.equal(data.get("text/plain"), "a");
  assert.equal(dataTransfer.effectAllowed, "move");
  assert.equal(view.row("a").props["data-dragging"], true);
  const over = event({ dataTransfer });
  view.row("c").props.onDragOver(over);
  view.render();
  assert.equal(over.defaultPrevented, true);
  assert.equal(dataTransfer.dropEffect, "move");
  assert.equal(view.row("c").props["data-drop-target"], true);
  assert.equal(view.drop("c").defaultPrevented, true);
  assert.deepEqual(view.ids(), ["b", "c", "a"]);
  assert.equal(view.row("a").props["data-dragging"], undefined);
  assert.equal(view.row("c").props["data-drop-target"], undefined);
  view.startDrag("a");
  view.drop("b");
  assert.deepEqual(home({ storage: view.storage }).ids(), ["a", "b", "c"]);
  assert.equal(view.storage.writes.length, 2);
  assert.deepEqual(view.opened, []);
});

test("cancelled drags and self/external drops do not change or persist the order", () => {
  const view = home();
  assert.equal(view.drop("b").defaultPrevented, false);
  const { dataTransfer } = view.startDrag("a");
  view.row("b").props.onDragOver(event({ dataTransfer }));
  view.render();
  view.handle("a").props.onDragEnd();
  view.render();
  assert.equal(view.row("a").props["data-dragging"], undefined);
  assert.equal(view.row("b").props["data-drop-target"], undefined);
  assert.equal(view.drop("b").defaultPrevented, false);
  view.startDrag("a");
  view.drop("a");
  assert.deepEqual(view.ids(), ["a", "b", "c"]);
  assert.equal(view.storage.writes.length, 0);
});

test("a project removed during a drag cannot be reinserted by dropping it", () => {
  const view = home();
  view.startDrag("a");
  view.render(projects.filter((project) => project.id !== "a"));
  view.drop("b");
  assert.deepEqual(view.ids(), ["b", "c"]);
  assert.equal(view.storage.writes.length, 0);
});

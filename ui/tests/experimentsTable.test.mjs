import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import ts from "typescript";

const require = createRequire(import.meta.url);
const jsx = (type, props) => ({ type, props });
let hookState = [];
let hookIndex = 0;
let effects = [];

const react = {
  useState(initialValue) {
    const index = hookIndex++;
    if (index >= hookState.length) {
      hookState[index] = typeof initialValue === "function" ? initialValue() : initialValue;
    }
    return [hookState[index], (value) => {
      hookState[index] = typeof value === "function" ? value(hookState[index]) : value;
    }];
  },
  useEffect(effect) {
    effects.push(effect);
  },
};

const messages = {
  m: new Proxy({}, { get: (_, name) => () => String(name) }),
};
const icons = new Proxy({}, { get: () => () => null });
const mocks = {
  "./WorkspaceEmptyState": { WorkspaceEmptyState: () => null },
  "../paraglide/messages.js": messages,
  "../i18n": { ltr: (value) => value },
  "lucide-react": icons,
  "../api": {
    fmtDuration: (milliseconds) => `duration:${milliseconds}`,
    fmtNumber: (value) => String(value),
    runDisplayStatus: () => "complete",
    timeAgo: (timestamp) => `relative:${timestamp}`,
  },
  "../tabPreview": { tabOpenGestureHandlers: () => ({}) },
  "./StatusBadge": { StatusBadge: ({ status }) => jsx("span", { children: status }) },
  "./ui": { Button: ({ children }) => jsx("button", { children }) },
  "./ArchiveMenu": { ArchiveMenu: () => null },
};
const source = readFileSync(new URL("../src/components/ExperimentsTable.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX },
}).outputText;
const exports = {};
new Function("require", "exports", compiled)((name) => {
  if (name === "react") return react;
  if (name === "react/jsx-runtime") return { jsx, jsxs: jsx, Fragment: Symbol("Fragment") };
  return mocks[name] ?? require(name);
}, exports);

function textContent(node) {
  if (Array.isArray(node)) return node.map(textContent).join("");
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (!node || typeof node !== "object") return "";
  if (typeof node.type === "function") return textContent(node.type(node.props ?? {}));
  return textContent(node.props?.children);
}

function renderTable(run) {
  hookIndex = 0;
  effects = [];
  return exports.ExperimentsTable({
    runs: run ? [run] : [],
    experiments: [{ id: "experiment-a", title: "Experiment A", slug: "experiment-a", branchName: "main", createdAt: 0 }],
    archiveActions: new Map([["experiment-a", {}]]),
    onOpen: () => {},
    onOpenLogs: () => {},
    onOpenCode: () => {},
    onArchive: () => {},
    onCancel: async () => {},
  });
}

test("a live run shows elapsed duration beside its relative time and refreshes each second", () => {
  const originalNow = Date.now;
  const originalWindow = globalThis.window;
  let now = 10_000;
  Date.now = () => now;
  hookState = [];
  try {
    const run = {
      id: "run-a",
      experimentId: "experiment-a",
      createdAt: 1_000,
      endedAt: null,
      status: "running",
      cancelRequested: false,
    };
    const first = textContent(renderTable(run));
    assert.match(first, /duration:9000/);
    assert.match(first, /relative:1000/);
    assert.equal(effects.length, 1);

    let tick;
    let interval;
    globalThis.window = {
      setInterval(callback, milliseconds) {
        tick = callback;
        interval = milliseconds;
        return 1;
      },
      clearInterval() {},
    };
    effects[0]();
    assert.equal(interval, 1_000);

    now += 1_000;
    tick();
    assert.match(textContent(renderTable(run)), /duration:10000/);
  } finally {
    Date.now = originalNow;
    if (originalWindow === undefined) delete globalThis.window;
    else globalThis.window = originalWindow;
  }
});

test("a completed run keeps its final duration instead of counting past completion", () => {
  const originalNow = Date.now;
  const originalWindow = globalThis.window;
  let intervalStarted = false;
  Date.now = () => 100_000;
  hookState = [];
  globalThis.window = {
    setInterval() {
      intervalStarted = true;
      return 1;
    },
    clearInterval() {},
  };
  try {
    const run = {
      id: "run-a",
      experimentId: "experiment-a",
      createdAt: 1_000,
      endedAt: 10_000,
      status: "done",
      cancelRequested: false,
    };
    const rendered = textContent(renderTable(run));
    assert.match(rendered, /duration:9000/);
    assert.equal(effects.length, 1);
    effects[0]();
    assert.equal(intervalStarted, false);
  } finally {
    Date.now = originalNow;
    if (originalWindow === undefined) delete globalThis.window;
    else globalThis.window = originalWindow;
  }
});

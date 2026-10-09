import { queryModules } from "./queryModules.mjs";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { describe, test } from "node:test";
import * as query from "@tanstack/react-query";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";

const require = createRequire(import.meta.url);
const en = JSON.parse(readFileSync(new URL("../messages/en.json", import.meta.url), "utf8"));
// English catalogue text with its placeholders filled, so assertions read as the UI does.
const messages = {
  m: new Proxy({}, {
    get: (_, key) => (inputs = {}) => {
      assert.ok(Object.hasOwn(en, key), `en.json has no message ${String(key)}`);
      return en[key].replace(/\{(\w+)\}/g, (_, name) => String(inputs[name]));
    },
  }),
};
const runtime = { getLocale: () => "en" };
const storage = new Map();
const localStorage = {
  getItem: (key) => storage.get(key) ?? null,
  setItem: (key, value) => void storage.set(key, value),
  removeItem: (key) => void storage.delete(key),
};

function load(file, mocks, globals = {}) {
  const source = readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8");
  // The file name tells the compiler whether `<T>` is a type argument or JSX.
  const code = ts.transpileModule(source, {
    fileName: file,
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  const exports = {};
  new Function("require", "exports", ...Object.keys(globals), code)(
    (name) => mocks[name] ?? require(name), exports, ...Object.values(globals),
  );
  return exports;
}

const i18n = load("i18n.ts", { "./paraglide/runtime.js": runtime });
const api = load("api.ts", {
  "./queries/client": {},
  "./queries/invalidation": {},
  "./paraglide/messages.js": messages,
  "./paraglide/runtime.js": runtime,
  "./i18n": i18n,
});
const model = load("components/experimentHistoryModel.ts", {
  "../api": api,
  "../i18n": i18n,
  "../paraglide/messages.js": messages,
});
const { load: loadQueries, client } = queryModules(api);
const { listChatSessionsQuery } = loadQueries("chat");
const icon = () => null;
const { ChapterHeader, ExperimentDetail, ExperimentHistory } = load(
  "components/ExperimentHistory.tsx",
  {
    "@tanstack/react-query": query,
    "lucide-react": { ChevronRight: icon, Copy: icon, GitBranch: icon, Search: icon, Terminal: icon, X: icon },
    "../api": api,
    "../i18n": i18n,
    "../paraglide/messages.js": messages,
    "../paraglide/runtime.js": runtime,
    "../queries/chat": { listChatSessionsQuery },
    "./Md": { Md: () => null },
    "./experimentHistoryModel": model,
  },
  { localStorage },
);
const { buildHistoryNodes, groupHistory } = model;

const project = { id: "project", name: "Example", baselineBranch: "trunk" };
client.setQueryData(listChatSessionsQuery(project.id).queryKey, [
  { id: "task-a", title: "Reproduce the paper" },
  { id: "task-b", title: "Resume handoff work" },
]);

function experiment(id, createdAt, parent, task, title) {
  return {
    id,
    projectId: project.id,
    parentExperimentId: parent,
    slug: id,
    branchName: "orx/" + id,
    title,
    description: "What changed in " + id,
    runCommand: "true",
    agentStatus: "idle",
    createdAt,
    updatedAt: createdAt,
    chatSessionId: task,
  };
}

function run(experimentId, createdAt, status = "done") {
  return { id: "run-" + experimentId + createdAt, experimentId, projectId: project.id, status, commitSha: "abc1234", createdAt, updatedAt: createdAt };
}

function render(experiments, runs, { emptyHint, showArchived = false } = {}) {
  return renderToStaticMarkup(
    React.createElement(
      query.QueryClientProvider,
      { client },
      React.createElement(ExperimentHistory, { project, experiments, runs, showArchived, emptyHint, onOpenChanges: () => {}, onOpenRun: () => {} }),
    ),
  );
}

function detail(node, overlay = false) {
  return renderToStaticMarkup(
    React.createElement(ExperimentDetail, {
      node,
      baselineBranch: project.baselineBranch,
      overlay,
      taskTitle: () => "",
      onSelect: () => {},
      onClose: () => {},
      onOpenChanges: () => {},
      onOpenRun: () => {},
    }),
  );
}

const rowIds = (html) => [...html.matchAll(/data-experiment="([^"]+)"/g)].map((match) => match[1]);

const tree = [
  experiment("baseline", 1, null, "task-a", "Worker pool baseline"),
  experiment("side-probe", 2, "baseline", "task-a", "A: 200 iterations"),
  experiment("c1-reproduce", 3, "baseline", "task-a", "C1: reproduce the paper"),
  experiment("f1-budget", 4, "c1-reproduce", "task-b", "F1: fixed-budget arms"),
  experiment("scratch", 5, null, null, "Scratch notes"),
];
const attempts = [run("baseline", 10), run("side-probe", 11, "failed"), run("c1-reproduce", 12), run("f1-budget", 13)];

describe("ExperimentHistory", () => {
  test("renders task chapters oldest first with loose experiments last", () => {
    storage.clear();

    const html = render(tree, attempts);
    const order = ["Reproduce the paper", "Resume handoff work", "Not in a task"].map((title) => html.indexOf(title));

    assert.equal(order.every((position, index) => position > 0 && (index === 0 || position > order[index - 1])), true);
  });

  test("draws the rail and lists side branches before the continuing child", () => {
    storage.clear();

    const html = render(tree, attempts);
    const rows = [...html.matchAll(/data-experiment="([^"]+)"/g)].map((match) => match[1]);

    assert.deepEqual({ rows, rails: html.split('class="pointer-events-none absolute left-0 top-0 z-10"').length - 1 }, {
      rows: ["baseline", "side-probe", "c1-reproduce", "f1-budget", "scratch"],
      rails: 3,
    });
  });

  test("names a parent from another task in a lead-in row", () => {
    storage.clear();

    const html = render(tree, attempts);

    assert.ok(html.includes('<span class="min-w-0 truncate">from \u2066c1-reproduce\u2069</span><span class="min-w-0 truncate text-muted">· Reproduce the paper</span>'));
  });

  test("shows status as a word beside its shape, never colour alone", () => {
    storage.clear();

    const html = render(tree, attempts);

    assert.equal(["Done", "Failed", "Not run"].every((word) => html.includes(`</svg>${word}</span>`)), true);
  });

  test("hides the rail and names parents in chronological order", () => {
    storage.clear();
    storage.set("orx:history-view", JSON.stringify({ grouping: "none", order: "newest" }));

    const html = render(tree, attempts);
    const rows = [...html.matchAll(/data-experiment="([^"]+)"/g)].map((match) => match[1]);

    assert.deepEqual({ rows, rail: html.includes("z-10"), parentLink: html.includes(">from C1</span>") }, {
      rows: ["scratch", "f1-budget", "c1-reproduce", "side-probe", "baseline"],
      rail: false,
      parentLink: true,
    });
  });

  test("folds a finished side branch of four experiments into one row", () => {
    storage.clear();
    const branch = [
      experiment("root", 1, null, "task-a", "Root"),
      experiment("s1", 2, "root", "task-a", "S1: side"),
      experiment("s2", 3, "s1", "task-a", "S2: side"),
      experiment("s3", 4, "s2", "task-a", "S3: side"),
      experiment("s4", 5, "s3", "task-a", "S4: side"),
      experiment("m1", 6, "root", "task-a", "M1: main"),
      experiment("m2", 7, "m1", "task-a", "M2: main"),
      experiment("m3", 8, "m2", "task-a", "M3: main"),
      experiment("m4", 9, "m3", "task-a", "M4: main"),
      experiment("m5", 10, "m4", "task-a", "M5: main"),
    ];

    const html = render(branch, branch.map((node, index) => run(node.id, 20 + index)));

    assert.ok(html.includes('<span class="text-text">4 more</span> · S1 → S4'));
  });

  test("says a missing parent is missing instead of calling the experiment a starting point", () => {
    storage.clear();
    storage.set("orx:history-view", JSON.stringify({ grouping: "none", order: "oldest" }));

    const html = render([experiment("orphan", 1, "gone", null, "Orphaned run")], []);

    assert.deepEqual({ missing: html.includes(">from a missing experiment</span>"), start: html.includes("starting point") }, { missing: true, start: false });
  });

  test("diffs a starting point against the project's baseline branch and disables the diff for a missing parent", () => {
    const [root, orphan] = buildHistoryNodes([experiment("root", 1, null, null, "Root"), experiment("orphan", 2, "gone", null, "Orphan")], []);
    const rootHtml = detail(root);
    const orphanHtml = detail(orphan);

    assert.deepEqual({
      rootDiff: rootHtml.includes("Diff from trunk"),
      rootMain: rootHtml.includes("Diff from main"),
      // Within one button: the current lineage step is a disabled button too.
      orphanDisabled: /<button[^>]*disabled=""[^>]*>(?:(?!<\/button>).)*No parent to diff/.test(orphanHtml),
    }, { rootDiff: true, rootMain: false, orphanDisabled: true });
  });

  test("lets a detail pane that covers the list take focus, and leaves one beside it out of the focus order", () => {
    const [node] = buildHistoryNodes([experiment("root", 1, null, null, "Root")], []);
    const aside = (html) => html.match(/<aside[^>]*>/)[0];

    assert.deepEqual({ covering: aside(detail(node, true)).includes('tabindex="-1"'), beside: aside(detail(node)).includes("tabindex") }, { covering: true, beside: false });
  });

  test("hides archived experiments unless asked and names an archived parent in a lead-in", () => {
    storage.clear();
    const archivedMiddle = [
      experiment("base", 1, null, "task-a", "Base"),
      { ...experiment("dead-end", 2, "base", "task-a", "Dead end"), archived: true },
      experiment("survivor", 3, "dead-end", "task-a", "Survivor"),
    ];
    const leadIn = '<span class="min-w-0 truncate">from \u2066dead-end\u2069</span>';

    const hidden = render(archivedMiddle, []);
    const shown = render(archivedMiddle, [], { showArchived: true });

    assert.deepEqual(
      { hidden: { rows: rowIds(hidden), leadIn: hidden.includes(leadIn) }, shown: { rows: rowIds(shown), leadIn: shown.includes(leadIn) } },
      { hidden: { rows: ["base", "survivor"], leadIn: true }, shown: { rows: ["base", "dead-end", "survivor"], leadIn: false } },
    );
  });

  test("shows the caller's hint when every experiment is archived", () => {
    storage.clear();
    const archived = [{ ...experiment("shelved", 1, null, "task-a", "Shelved"), archived: true }];

    const html = render(archived, [], { emptyHint: "Every experiment is archived." });

    assert.deepEqual({ hint: html.includes("Every experiment is archived."), rows: rowIds(html) }, { hint: true, rows: [] });
  });

  test("drops a chapter's status counts and dates when the list is too narrow for them", () => {
    const noon = Date.UTC(2026, 9, 3, 12);
    const [group] = groupHistory(buildHistoryNodes([experiment("solo", noon, null, "task-a", "Solo")], [run("solo", noon)]), "task", () => "Reproduce the paper");
    const text = (showSummary) =>
      renderToStaticMarkup(React.createElement(ChapterHeader, { group, open: true, showSummary, onToggle: () => {} })).replace(/<[^>]+>/g, "");

    assert.deepEqual({ wide: text(true), narrow: text(false) }, { wide: "Reproduce the paper" + "1" + "1 done · Oct 3", narrow: "Reproduce the paper" + "1" });
  });

  test("shows the caller's hint when there are no experiments", () => {
    storage.clear();

    assert.ok(render([], [], { emptyHint: "Nothing here yet." }).includes("Nothing here yet."));
  });
});

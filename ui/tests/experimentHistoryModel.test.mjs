import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { describe, test } from "node:test";
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

function load(file, mocks) {
  const source = readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8");
  const code = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const exports = {};
  new Function("require", "exports", code)((name) => mocks[name] ?? require(name), exports);
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
const {
  buildHistoryNodes,
  describeAttemptChange,
  diffBaseOf,
  groupHistory,
  historyBackendLabel,
  historyFit,
  historyJobId,
  laneCount,
  layoutChronological,
  layoutLineage,
  originOf,
  splitExperimentTitle,
} = load("components/experimentHistoryModel.ts", {
  "../api": api,
  "../i18n": i18n,
  "../paraglide/messages.js": messages,
});

function experiment(id, createdAt, parentExperimentId = null, chatSessionId = "session-a", title = "Change " + id) {
  return {
    id,
    projectId: "project",
    parentExperimentId,
    slug: id,
    branchName: "orx/" + id,
    title,
    description: "Detail " + id,
    runCommand: "true",
    agentStatus: "idle",
    createdAt,
    updatedAt: createdAt,
    chatSessionId,
  };
}

function run(id, experimentId, createdAt, commitSha, status = "done") {
  return {
    id,
    experimentId,
    projectId: "project",
    status,
    commitSha,
    createdAt,
    updatedAt: createdAt,
  };
}

/** One chapter holding every node, as the "none" grouping produces. */
function chapter(experiments, runs = []) {
  return groupHistory(buildHistoryNodes(experiments, runs), "none", () => undefined)[0];
}

/** Rows as [kind, experiment id, lane] so a whole layout reads in one assertion. */
function shape(rows) {
  return rows.map((row) => [row.kind, row.kind === "collapsed" ? row.head.id : row.node.id, row.lane]);
}

/** A value interpolated as an isolated left-to-right run, as `ltr` renders it. */
const isolated = (value) => `\u2066${value}\u2069`;

const allDone = (ids) => ids.map((id, index) => run("run-" + id, id, 100 + index, "abc1234"));

describe("splitExperimentTitle", () => {
  test("separates a leading experiment code from a sentence-case label", () => {
    const cases = [
      ["D2: re-score C2 at higher n", { code: "D2", label: "Re-score C2 at higher n" }],
      ["A: 200 iterations", { code: "A", label: "200 iterations" }],
      ["G3-probe: does a pool help?", { code: "G3", label: "Probe: does a pool help?" }],
      ["I6 probe: a vocabulary", { code: "I6", label: "Probe: a vocabulary" }],
      ["Ablation E: random routing", { code: null, label: "Ablation E: random routing" }],
      ["PPO baseline (small model)", { code: null, label: "PPO baseline (small model)" }],
    ];

    const parsed = cases.map(([title]) => splitExperimentTitle(experiment("x", 1, null, null, title)));

    assert.deepEqual(parsed, cases.map(([, expected]) => expected));
  });

  test("falls back to the slug when there is no title or description", () => {
    const bare = { ...experiment("bare-slug", 1, null, null, null), description: null };

    assert.deepEqual(splitExperimentTitle(bare), { code: null, label: "Bare-slug" });
  });
});

describe("buildHistoryNodes", () => {
  test("orders attempts oldest first and takes the status from the latest", () => {
    const nodes = buildHistoryNodes(
      [experiment("root", 1), experiment("child", 2, "root")],
      [run("late", "child", 30, "bbb", "failed"), run("early", "child", 10, "aaa", "done")],
    );
    const child = nodes.find((node) => node.id === "child");

    assert.deepEqual(
      {
        runs: child.runs.map((attempt) => attempt.id),
        status: child.status,
        parent: child.parent?.id,
        rootStatus: nodes[0].status,
      },
      { runs: ["early", "late"], status: "failed", parent: "root", rootStatus: "none" },
    );
  });

  test("lays out an experiment whose parent is not in the list as a root but keeps the missing id", () => {
    const [orphan] = buildHistoryNodes([experiment("orphan", 1, "gone")], []);

    assert.deepEqual({ parent: orphan.parent, missingParentId: orphan.missingParentId }, { parent: null, missingParentId: "gone" });
  });
});

describe("origin and diff base", () => {
  test("names the parent for a child experiment", () => {
    const [, child] = buildHistoryNodes([experiment("root", 1, null, "session-a", "A: baseline"), experiment("child", 2, "root")], []);

    assert.deepEqual({ origin: originOf(child), base: diffBaseOf(child, "trunk") }, { origin: "from A", base: "A" });
  });

  test("compares a starting point with the project's baseline branch", () => {
    const [root] = buildHistoryNodes([experiment("root", 1)], []);

    assert.deepEqual({ origin: originOf(root), base: diffBaseOf(root, "trunk") }, { origin: "starting point", base: "trunk" });
  });

  test("offers no diff base when the recorded parent is missing", () => {
    const [orphan] = buildHistoryNodes([experiment("orphan", 1, "gone")], []);

    assert.deepEqual({ origin: originOf(orphan), base: diffBaseOf(orphan, "trunk") }, { origin: "from a missing experiment", base: null });
  });
});

describe("groupHistory", () => {
  const nodes = buildHistoryNodes(
    [
      experiment("late-task", 5, null, "session-b"),
      experiment("loose", 1, null, null),
      experiment("early-task", 2, null, "session-a"),
      experiment("deleted-chat", 3, null, "session-gone"),
    ],
    [],
  );
  const titles = new Map([
    ["session-a", "Reproduce the paper"],
    ["session-b", "Resume handoff work"],
  ]);

  test("makes one chapter per agent task, oldest first, with loose experiments last", () => {
    const chapters = groupHistory(nodes, "task", (task) => titles.get(task));

    assert.deepEqual(chapters.map((group) => [group.title, group.nodes.map((node) => node.id)]), [
      ["Reproduce the paper", ["early-task"]],
      ["Untitled task", ["deleted-chat"]],
      ["Resume handoff work", ["late-task"]],
      ["Not in a task", ["loose"]],
    ]);
  });

  test("groups by lineage root and names the chapter after the root", () => {
    const tree = buildHistoryNodes(
      [experiment("r", 1, null, null, "C1: reproduce"), experiment("k", 2, "r", null), experiment("solo", 3)],
      [],
    );

    const chapters = groupHistory(tree, "lineage", () => undefined);

    assert.deepEqual(chapters.map((group) => [group.title, group.nodes.map((node) => node.id)]), [
      ["C1 · Reproduce", ["r", "k"]],
      ["Change solo", ["solo"]],
    ]);
  });
});

describe("layoutLineage", () => {
  test("lists side branches before the continuing child, which keeps its parent's lane", () => {
    const group = chapter([
      experiment("p", 1),
      experiment("side", 2, "p"),
      experiment("main", 3, "p"),
      experiment("main-2", 4, "main"),
    ]);

    assert.deepEqual(shape(layoutLineage(group)), [
      ["experiment", "p", 0],
      ["experiment", "side", 1],
      ["experiment", "main", 0],
      ["experiment", "main-2", 0],
    ]);
  });

  test("keeps a six-way fork of leaves to two lanes", () => {
    const leaves = ["a", "b", "c", "d", "e", "f"].map((id, index) => experiment(id, 2 + index, "p"));

    const rows = layoutLineage(chapter([experiment("p", 1), ...leaves]));

    assert.equal(laneCount(rows), 2);
  });

  test("continues through the child that forks, so a plain chain moves aside instead of a third lane", () => {
    const group = chapter([
      experiment("p", 1),
      experiment("forks", 2, "p"),
      experiment("forks-a", 3, "forks"),
      experiment("forks-b", 4, "forks"),
      experiment("chain", 5, "p"),
      experiment("chain-2", 6, "chain"),
      experiment("chain-3", 7, "chain-2"),
    ]);

    assert.deepEqual(shape(layoutLineage(group)), [
      ["experiment", "p", 0],
      ["experiment", "chain", 1],
      ["experiment", "chain-2", 1],
      ["experiment", "chain-3", 1],
      ["experiment", "forks", 0],
      ["experiment", "forks-a", 1],
      ["experiment", "forks-b", 0],
    ]);
  });

  test("opens a third lane only when both branches of a fork fork again", () => {
    const group = chapter([
      experiment("p", 1),
      experiment("x", 2, "p"),
      experiment("xa", 3, "x"),
      experiment("xb", 4, "x"),
      experiment("y", 5, "p"),
      experiment("ya", 6, "y"),
      experiment("yb", 7, "y"),
    ]);

    assert.deepEqual(shape(layoutLineage(group)), [
      ["experiment", "p", 0],
      ["experiment", "x", 1],
      ["experiment", "xa", 2],
      ["experiment", "xb", 1],
      ["experiment", "y", 0],
      ["experiment", "ya", 1],
      ["experiment", "yb", 0],
    ]);
  });

  test("precedes an experiment whose parent is in another chapter with a lead-in", () => {
    const nodes = buildHistoryNodes([experiment("base", 1, null, "a"), experiment("next", 2, "base", "b")], []);
    const [, second] = groupHistory(nodes, "task", () => undefined);

    const rows = layoutLineage(second);

    assert.deepEqual(rows.map((row) => (row.kind === "lead-in" ? ["lead-in", row.from.id] : [row.kind, row.lane])), [
      ["lead-in", "base"],
      ["experiment", 0],
    ]);
  });

  describe("collapsing finished side branches", () => {
    const ids = ["side", "side-2", "side-3", "side-4"];
    const tree = [
      experiment("p", 1),
      experiment("side", 2, "p"),
      experiment("side-2", 3, "side"),
      experiment("side-3", 4, "side-2"),
      experiment("side-4", 5, "side-3"),
      experiment("main", 6, "p"),
      experiment("main-2", 7, "main"),
      experiment("main-3", 8, "main-2"),
      experiment("main-4", 9, "main-3"),
      experiment("main-5", 10, "main-4"),
    ];

    test("folds a branch of four finished experiments into one row", () => {
      const rows = layoutLineage(chapter(tree, allDone(ids)), { collapseFinished: true });

      assert.deepEqual(shape(rows).slice(0, 3), [
        ["experiment", "p", 0],
        ["collapsed", "side", 1],
        ["experiment", "main", 0],
      ]);
    });

    test("leaves a branch open while any of it has not finished successfully", () => {
      const runs = [...allDone(ids.slice(0, 3)), run("fail", "side-4", 200, "abc", "failed")];

      const rows = layoutLineage(chapter(tree, runs), { collapseFinished: true });

      assert.equal(rows.some((row) => row.kind === "collapsed"), false);
    });

    test("leaves a branch open when it holds the selection or was expanded", () => {
      const group = chapter(tree, allDone(ids));

      const kept = layoutLineage(group, { collapseFinished: true, keep: new Set(["side-3"]) });
      const expanded = layoutLineage(group, { collapseFinished: true, expanded: new Set(["side"]) });

      assert.deepEqual([kept, expanded].map((rows) => rows.some((row) => row.kind === "collapsed")), [false, false]);
    });

    test("never folds a branch shorter than four experiments", () => {
      const short = tree.filter((node) => node.id !== "side-4");

      const rows = layoutLineage(chapter(short, allDone(ids.slice(0, 3))), { collapseFinished: true });

      assert.equal(rows.some((row) => row.kind === "collapsed"), false);
    });

    test("leaves out of the fold a later experiment reached through another chapter, which starts its own root", () => {
      const handedOff = [...tree, experiment("handoff", 11, "side-4", "session-b"), experiment("resumed", 12, "handoff")];
      const nodes = buildHistoryNodes(handedOff, allDone([...ids, "handoff", "resumed"]));
      const [own] = groupHistory(nodes, "task", () => undefined);

      const rows = layoutLineage(own, { collapseFinished: true });

      assert.deepEqual(rows.map((row) => (row.kind === "collapsed" ? ["collapsed", row.hidden.map((node) => node.id)] : [row.kind, row.node.id])), [
        ["experiment", "p"],
        ["collapsed", ["side", "side-2", "side-3", "side-4"]],
        ["experiment", "main"],
        ["experiment", "main-2"],
        ["experiment", "main-3"],
        ["experiment", "main-4"],
        ["experiment", "main-5"],
        ["lead-in", "resumed"],
        ["experiment", "resumed"],
      ]);
    });
  });
});

describe("historyFit", () => {
  test("sets the open detail pane beside a wide list, which keeps its focus", () => {
    assert.deepEqual(historyFit(1200, true, false), {
      overlay: false,
      listCovered: false,
      whatChanged: true,
      from: true,
      runs: true,
      chapterSummary: true,
    });
  });

  test("covers a narrow list with the open detail pane and takes the list out of the focus order", () => {
    assert.deepEqual([historyFit(900, true, false), historyFit(900, false, false)].map(({ overlay, listCovered }) => ({ overlay, listCovered })), [
      { overlay: true, listCovered: true },
      { overlay: true, listCovered: false },
    ]);
  });

  test("keeps only the experiment and its status in a compact list, in any order", () => {
    const compact = { overlay: true, listCovered: false, whatChanged: false, from: false, runs: false, chapterSummary: false };

    assert.deepEqual([historyFit(400, false, false), historyFit(400, false, true)], [compact, compact]);
  });

  test("names the parent in a From column only outside lineage order", () => {
    assert.deepEqual([historyFit(600, false, false).from, historyFit(600, false, true).from], [true, false]);
  });
});

describe("layoutChronological", () => {
  test("lists experiments by creation time, newest first on request", () => {
    const group = chapter([experiment("b", 2), experiment("a", 1, "b"), experiment("c", 3)]);

    assert.deepEqual([layoutChronological(group), layoutChronological(group, true)].map(shape), [
      [["experiment", "a", 0], ["experiment", "b", 0], ["experiment", "c", 0]],
      [["experiment", "c", 0], ["experiment", "b", 0], ["experiment", "a", 0]],
    ]);
  });
});

describe("run attempt metadata", () => {
  test("distinguishes initial attempts, retries and new code", () => {
    const first = run("first", "baseline", 1, "aaaaaaa");
    const retry = run("retry", "baseline", 2, "aaaaaaa");
    const changed = run("changed", "baseline", 3, "bbbbbbb");

    assert.equal(describeAttemptChange(first, null), "Initial attempt");
    assert.equal(describeAttemptChange(retry, first), "Same commit · retry");
    assert.equal(describeAttemptChange(changed, retry), `New commit · ${isolated("bbbbbbb")}`);
  });

  test("does not invent a comparison when either commit is missing", () => {
    const unknown = run("unknown", "baseline", 1, null);
    const known = run("known", "baseline", 2, "bbbbbbb");

    assert.equal(describeAttemptChange(known, unknown), "Code provenance unavailable");
    assert.equal(describeAttemptChange(unknown, known), "Code provenance unavailable");
  });

  test("renders backend detail and scheduler job id without inventing fields", () => {
    const attempt = {
      ...run("slurm", "baseline", 1, "aaaaaaa"),
      backend: {
        kind: "slurm_job",
        namespace: "research-cluster",
        flavor: "h100:1",
        jobId: 4336055,
      },
    };

    assert.equal(historyBackendLabel(attempt), "slurm_job · h100:1");
    assert.equal(historyJobId(attempt), "4336055");
    assert.equal(historyJobId(run("local", "baseline", 2, null)), "—");
  });
});

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import { resolveHarnessDefaultModel } from "../src/modelSelection.ts";

const source = (path) => ts.createSourceFile(path, readFileSync(new URL(`../src/${path}`, import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
function evaluate(code, bindings) {
  const js = ts.transpileModule(code, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  return new Function(...Object.keys(bindings), js)(...Object.values(bindings));
}
function find(file, predicate) {
  const matches = [];
  function visit(node) {
    if (predicate(node)) matches.push(node);
    ts.forEachChild(node, visit);
  }
  visit(file);
  return matches;
}

const form = source("components/NewProjectForm.tsx");
const component = find(form, (node) => ts.isFunctionDeclaration(node) && node.name?.text === "NewProjectForm")[0];
const statements = component.body.statements;
const firstState = statements.findIndex((node) => node.getText(form).includes("useState<"));
const selection = statements.slice(0, firstState).map((node) => node.getText(form)).join("\n");
const submit = find(form, (node) => ts.isFunctionDeclaration(node) && node.name?.text === "submit")[0];
const prewarmEffect = statements.find((node) => ts.isExpressionStatement(node) && node.getText(form).includes("prewarmStarterPrompts("));
const picker = source("components/ModelPicker.tsx");
const fallback = find(picker, (node) => ts.isFunctionDeclaration(node) && node.name?.text === "defaultSelection")[0];
const defaultSelection = evaluate(`return ${fallback.getText(picker).replace("export ", "")}`, {
  resolveHarnessDefaultModel,
  reconcileServiceTier: () => null,
  reasoningFor: () => ({ defaultId: null }),
});

test("project creation and debounced prewarm use the current selection, including custom and null OpenCode models", async () => {
  const stale = { harness: "codex", model: "saved-model" };
  let dependencies, cleanup;
  const timers = new Map();
  let timerId = 0;
  const created = [], warmed = [];
  const harnesses = [{ id: "codex", agentReady: true, models: [{ id: "catalog-default" }] }];
  const bindings = {
    useQuery: () => ({ data: harnesses }), getHarnessesQuery: () => ({}), defaultSelection,
    useEffect: (effect, next) => {
      if (dependencies && next.every((value, i) => Object.is(value, dependencies[i]))) return;
      cleanup?.();
      dependencies = next;
      cleanup = effect();
    },
    window: {
      setTimeout: (callback) => { timers.set(++timerId, callback); return timerId; },
      clearTimeout: (id) => timers.delete(id),
    },
    prewarmReady: true, prewarmName: "Project", prewarmPaperId: null, prewarmPath: "/project",
    prewarmStarterPrompts: async (request) => { warmed.push(request); },
    queryClient: { fetchQuery: async () => ({ exists: true, directory: true }) },
    getProjectPathStatusQuery: () => ({}), getLocale: () => "en",
    createMutation: { mutateAsync: async (request) => { created.push(request); return { project: {}, githubPublicationError: null }; } },
    canCreate: true, projectPath: "/project", name: "Project", mode: "folder", paper: null,
    githubSyncEnabled: false, existingGithubRepo: null,
    setPending: () => {}, setError: (error) => { assert.equal(error, null); }, onCreated: () => {},
    persistedPreferredAgent: { current: stale },
  };
  const render = (preferredAgent) => evaluate(`${selection}\n${submit.getText(form)}\n${prewarmEffect.getText(form)}\nreturn submit;`, { ...bindings, preferredAgent });
  const custom = { harness: "opencode", model: "my-provider/custom-model", permissionMode: null, reasoningLevel: null };
  render(stale);
  render(custom);
  assert.equal(timers.size, 1, "changing selection replaces the pending prewarm");
  render({ ...custom });
  assert.equal(timerId, 2, "an equivalent selection does not restart the debounce");
  for (const expected of [custom, { harness: "opencode", model: null }, { harness: "claude-code", model: null }, null]) {
    const send = render(expected);
    for (const [id, callback] of timers) { timers.delete(id); callback(); }
    await send({ preventDefault() {} });
    const agent = expected ? { harness: expected.harness, model: expected.model } : { harness: "codex", model: "catalog-default" };
    assert.deepEqual(created.at(-1).starterAgent, agent);
    assert.deepEqual(warmed.at(-1).starterAgent, agent);
  }
});

test("project entry points pass their current preference through to the form", () => {
  const current = { harness: "opencode", model: "my-provider/current" };
  const stale = { harness: "codex", model: "saved-model" };
  for (const [path, tag, bindings] of [
    ["App.tsx", "NewProjectDialog", { uiState: { preferredAgent: current }, persistedPreferredAgent: { current: stale } }],
    ["routePages.tsx", "ProjectsEmptyState", { state: { preferredAgent: current } }],
    ["components/ProjectsHome.tsx", "NewProjectForm", { preferredAgent: current }],
  ]) {
    const file = source(path);
    const elements = find(file, (node) => (ts.isJsxSelfClosingElement(node) || ts.isJsxOpeningElement(node)) && node.tagName.getText(file) === tag);
    assert.ok(elements.length, `${path}: ${tag}`);
    for (const element of elements) {
      const prop = element.attributes.properties.find((node) => ts.isJsxAttribute(node) && node.name.getText(file) === "preferredAgent");
      assert.ok(prop?.initializer && ts.isJsxExpression(prop.initializer), `${path}: current preference`);
      assert.equal(evaluate(`return ${prop.initializer.expression.getText(file)}`, bindings), current);
    }
  }
});

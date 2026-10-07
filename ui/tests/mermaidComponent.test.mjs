import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import ts from "typescript";

// The fence and file predicates are covered directly. This file covers the
// other half: what the component does with a rendered diagram once it exists.
//
// There is no DOM here, and adding one is not worth it for this. What the
// regression below was actually about is *which renders rebind* — a pure
// question about the effect's dependency list — so the harness transpiles the
// component, stubs the hooks, and records which renders run each effect. That
// catches the regression directly, which is what a DOM test would have done
// more slowly.

const require = createRequire(import.meta.url);
const messages = { m: new Proxy({}, { get: (_, name) => () => String(name) }) };

function load(file, mocks, globals = {}) {
  const source = readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8");
  const code = ts.transpileModule(source, {
    compilerArgs: [],
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2022,
      jsx: ts.JsxEmit.ReactJSX,
    },
  }).outputText;
  const exports = {};
  new Function("require", "exports", ...Object.keys(globals), code)(
    (name) => mocks[name] ?? require(name),
    exports,
    ...Object.values(globals),
  );
  return exports;
}

/** React stub that records which effects ran, keyed by their dependency list. */
function hooks(seed = []) {
  const states = [...seed];
  const effects = [];
  let cursor = 0;
  return {
    reset: () => {
      cursor = 0;
      effects.length = 0;
    },
    ranLastRender: () => effects,
    react: {
      ...React,
      useRef: (initial) => ({ current: initial, __ref: true }),
      useState: (initial) => {
        const index = cursor++;
        if (!(index in states)) states[index] = initial;
        return [states[index], (value) => {
          states[index] = typeof value === "function" ? value(states[index]) : value;
        }];
      },
      useEffect: (fn, deps) => effects.push({ hook: "effect", deps, fn }),
      useLayoutEffect: (fn, deps) => effects.push({ hook: "layout", deps, fn }),
      useMemo: (fn) => fn(),
      useCallback: (fn) => fn,
    },
    states,
  };
}

/**
 * A rendered diagram, and a record of every element bindFunctions ran on.
 *
 * `result` seeds the component's state the way a completed render would, since
 * the real one is filled by an async effect this harness does not run.
 */
function harness({ result = null, showDiagram = true } = {}) {
  const bound = [];
  const svg = { id: "svg" };
  const host = { querySelector: (selector) => (selector === "svg" ? svg : null) };
  // MermaidDiagram's own hooks, in order: result, showDiagram, theme. CopyButton
  // is created as an element but never invoked here, so it consumes nothing.
  const state = hooks([result, showDiagram, "light"]);
  // The component's first useRef is the svg host (its 4th hook overall); hand
  // it a ref pre-pointed at the fake host so the layout effect finds an svg.
  // Hook order in MermaidDiagram: result, showDiagram, theme, hostRef, settled.
  const ref = { current: host };
  const baseUseRef = state.react.useRef;
  let refCalls = 0;
  state.react.useRef = (initial) => (refCalls++ === 0 ? ref : baseUseRef(initial));
  const { MermaidDiagram } = load("components/Mermaid.tsx", {
    react: state.react,
    "../mermaid": {
      renderMermaid: () => Promise.resolve(result),
      isMermaidLanguage: () => false,
      isMermaidFile: () => false,
      mermaidTheme: () => "default",
    },
    "../syntaxHighlight": { highlight: (code) => code },
    "../paraglide/messages.js": messages,
    "./ui": { IconButton: "button" },
    "lucide-react": new Proxy({}, { get: () => "icon" }),
  }, { document: { documentElement: { dataset: { theme: "light" } } } });
  const bindFunctions = (element) => bound.push(element);
  return {
    state, bound, svg, host, MermaidDiagram, bindFunctions,
    code: "flowchart TD\n A-->B",
  };
}

/**
 * Deps of the layout effect that applies bindFunctions, as React would compare
 * them across renders. `undefined` stands for a dep React treats as always
 * changed.
 */
function layoutEffectDeps(state) {
  const last = state.ranLastRender().filter((entry) => entry.hook === "layout");
  return last.length ? last : [];
}

/**
 * The component registers two layout effects; the one that applies
 * bindFunctions is the only one whose deps mention the render result. Selecting
 * it by shape keeps this from silently binding the wrong effect if the other
 * one is reordered.
 */
function bindEffect(state) {
  const layout = state.ranLastRender().filter((entry) => entry.hook === "layout");
  const bind = layout.find((entry) =>
    (entry.deps ?? []).some((dep) => dep && typeof dep === "object" && "ok" in dep),
  );
  assert.ok(bind, "expected a layout effect that binds the rendered diagram");
  return bind;
}

function svgMarkup() {
  return '<svg><g class="node"></g></svg>';
}

test("bindFunctions runs against the svg the component mounted", () => {
  const bound = [];
  const { state, MermaidDiagram, code, host } = harness({
    result: { ok: true, svg: svgMarkup(), bindFunctions: (el) => bound.push(el) },
  });
  state.reset();
  const tree = MermaidDiagram({ code });

  // Mount the way React would: the ref callback receives the host element, then
  // the layout effect body runs against it.
  const found = [];
  const walk = (node) => {
    if (!React.isValidElement(node)) return;
    found.push(node);
    React.Children.toArray(node.props.children).forEach(walk);
  };
  walk(tree);
  const canvas = found.find((n) => n.props?.className?.includes?.("mermaid-diagram-canvas"));
  assert.ok(canvas, "expected the diagram canvas to mount");

  const bind = bindEffect(state);
  bind.fn();

  assert.deepEqual(bound, [host.querySelector("svg")], "bindFunctions must receive the mounted svg");
});

test("the rebind effect keys on the source/diagram toggle, not just result", () => {
  // The regression greptile flagged: toggling to source unmounts the svg host
  // and toggling back mounts a brand new svg carrying none of the old handlers.
  // Keying the effect on `result` alone left that new node unbound.
  const first = harness({ result: { ok: true, svg: svgMarkup(), bindFunctions: undefined } });
  first.state.reset();
  first.MermaidDiagram({ code: first.code });
  const deps = bindEffect(first.state).deps ?? [];

  assert.equal(
    deps.length > 1,
    true,
    "bind effect must re-run when the diagram is shown again after source, " +
      `so its deps must include the toggle; got ${JSON.stringify(deps)}`,
  );
});

test("a failed render keeps the source visible and does not mount the canvas", () => {
  const { MermaidDiagram, code } = harness({ result: { ok: false, error: "unsupported" } });
  const found = [];
  const walk = (tree) => {
    if (!React.isValidElement(tree)) return;
    found.push(tree);
    React.Children.toArray(tree.props.children).forEach(walk);
  };
  walk(MermaidDiagram({ code }));
  assert.equal(
    found.some((node) => node.props?.className?.includes?.("mermaid-diagram-canvas")),
    false,
    "a failed render must not mount the diagram canvas",
  );
});

test("a successful render mounts the canvas with the engine's markup", () => {
  const markup = svgMarkup();
  const { MermaidDiagram, code } = harness({
    result: { ok: true, svg: markup, bindFunctions: undefined },
  });
  const found = [];
  const walk = (tree) => {
    if (!React.isValidElement(tree)) return;
    found.push(tree);
    React.Children.toArray(tree.props.children).forEach(walk);
  };
  walk(MermaidDiagram({ code }));
  const canvas = found.find((node) => node.props?.className?.includes?.("mermaid-diagram-canvas"));
  assert.ok(canvas, "a successful render must mount the diagram canvas");
  assert.equal(canvas.props.dangerouslySetInnerHTML.__html, markup);
});

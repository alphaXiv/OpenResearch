import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";

import {
  isMermaidFile,
  isMermaidLanguage,
  mermaidDeclarationLine,
  mermaidHasContent,
} from "../src/mermaid.ts";

// The engine needs a DOM, so only the pure decision layer is covered here:
// which fences and files are treated as diagrams, and which blocks are worth
// loading the engine for. Everything past that gate is mermaid's own parser.

test("only a mermaid fence info string asks for a diagram", () => {
  for (const language of ["mermaid", "Mermaid", " MERMAID "]) {
    assert.equal(isMermaidLanguage(language), true, language);
  }
  for (const language of ["", "mermaid-extra", "mmd", "javascript", "python", null, undefined]) {
    assert.equal(isMermaidLanguage(language), false, String(language));
  }
});

test("standalone diagram files are recognized by extension alone", () => {
  for (const name of ["flow.mmd", "flow.mermaid", "Flow.MMD", "artifacts/plan.mmd"]) {
    assert.equal(isMermaidFile(name), true, name);
  }
  for (const name of ["flow.md", "flow.mmd.txt", "mmd", "flow.markdown", "flow.mmdx", "mermaid"]) {
    assert.equal(isMermaidFile(name), false, name);
  }
});

test("the declaration is the first line that is not blank or a mermaid comment", () => {
  assert.equal(mermaidDeclarationLine("flowchart TD\n  A-->B"), "flowchart TD");
  assert.equal(mermaidDeclarationLine("\n\n%% a note\n%% another\n  graph LR"), "graph LR");
  assert.equal(
    mermaidDeclarationLine("   \n%%{init: {'theme':'dark'}}%%\nsequenceDiagram"),
    "sequenceDiagram",
  );
  assert.equal(mermaidDeclarationLine("%% only comments\n%% more"), "");
  assert.equal(mermaidDeclarationLine(""), "");
});

test("a block with only whitespace and comments is skipped before the engine loads", () => {
  for (const code of ["", "   ", "\n\n\t\n", "%% a diagram goes here\n%% TODO", "%%"]) {
    assert.equal(mermaidHasContent(code), false, JSON.stringify(code));
  }
});

test("anything with a line in it is handed to the engine to judge", () => {
  // The gate deliberately does not try to name the diagram type: mermaid
  // registers a detector per diagram at runtime, so a local keyword list would
  // go stale. A block like "A --> B" is a headerless flowchart body and only
  // mermaid can say whether it parses.
  for (const code of [
    "flowchart TD\n A-->B",
    "sequenceDiagram\n A->>B: hi",
    "stateDiagram-v2\n [*] --> S",
    "mindmap\n root((a))",
    "A --> B",
    "%% a comment\n\nflowchart LR\n A-->B",
  ]) {
    assert.equal(mermaidHasContent(code), true, JSON.stringify(code));
  }
});

// Walks the hast for a <pre><code class="language-…"> so the assertions do not
// depend on where remark-rehype puts its inter-element whitespace.
function fencedBlock(tree) {
  const pre = tree.children.find(
    (node) => node.tagName === "pre" && node.children?.[0]?.tagName === "code",
  );
  return pre?.children[0];
}

const processor = unified().use(remarkParse).use(remarkRehype);

test("a ```mermaid fence reaches the renderer as a language-mermaid code block", () => {
  const tree = processor.runSync(
    processor.parse("Before\n\n```mermaid\nflowchart TD\n  A-->B\n```\n\nAfter\n"),
  );
  const code = fencedBlock(tree);
  assert.ok(code.properties.className.includes("language-mermaid"));
  assert.equal(code.children[0].value.trim(), "flowchart TD\n  A-->B");
  assert.equal(isMermaidLanguage(code.properties.className[0].slice("language-".length)), true);
});

test("a fence with no info string is not routed to the diagram renderer", () => {
  const code = fencedBlock(processor.runSync(processor.parse("```\nflowchart TD\n  A-->B\n```")));
  assert.equal(code.properties.className, undefined);
  assert.equal(isMermaidLanguage(code.properties.className?.[0]), false);
});

test("a half-streamed mermaid fence still arrives as a diagram block", () => {
  // An unterminated fence has no closing backticks, which is what every
  // in-flight chat message looks like. The renderer gets the same code node, so
  // the diagram is attempted and simply falls back to source until it completes.
  const code = fencedBlock(processor.runSync(processor.parse("```mermaid\nflowchart TD\n  A-->")));
  assert.ok(code.properties.className.includes("language-mermaid"));
  assert.equal(mermaidHasContent(code.children[0].value), true);
});

test("a half-streamed fence that is still empty falls back without loading", () => {
  const code = fencedBlock(processor.runSync(processor.parse("```mermaid")));
  assert.ok(code.properties.className.includes("language-mermaid"));
  assert.equal(mermaidHasContent(code.children[0]?.value ?? ""), false);
});
// A diagram can carry its own `%%{init: {...}}%%` directive, and mermaid applies
// it over the app's initialize() config unless the key is listed in `secure`.
// htmlLabels is the one that matters: re-enabling it restores the foreignObject
// label path, and DOMPurify permits <img src>, so an author-chosen URL would be
// fetched when the SVG is inserted. This asserts the lock exists in source, since
// the escape itself is only observable with a real DOM.
test("htmlLabels is pinned in the secure list, not merely defaulted", () => {
  const source = readFileSync(new URL("../src/mermaid.ts", import.meta.url), "utf8");
  const block = /mermaid\.initialize\(\{[\s\S]*?\}\);/.exec(source)?.[0];
  assert.ok(block, "expected an initialize() call");
  assert.match(block, /htmlLabels:\s*false/, "htmlLabels must default to false");
  const secure = /secure:\s*\[([^\]]*)\]/.exec(block);
  assert.ok(secure, "initialize() must pass a secure list");
  const keys = secure[1].split(",").map((k) => k.trim().replace(/^"|"$/g, ""));
  assert.ok(
    keys.includes("htmlLabels"),
    `htmlLabels must be in secure so a %%{init} directive cannot re-enable it, got ${JSON.stringify(keys)}`,
  );
  assert.ok(keys.includes("securityLevel"), "securityLevel must stay pinned too");
});

// The engine loader is the one piece of module state worth asserting on, because
// caching a *failed* import would leave every diagram in source fallback for the
// rest of the session. Transpiled with the dynamic import stubbed so the failure
// and the retry can both be observed without a DOM.
test("a failed engine import is not cached, so the next diagram retries", async () => {
  const { readFileSync } = await import("node:fs");
  const ts = (await import("typescript")).default;
  const source = readFileSync(new URL("../src/mermaid.ts", import.meta.url), "utf8");
  const code = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;

  let attempts = 0;
  const engine = { initialize() {}, render: async () => ({ svg: "<svg></svg>" }), parseError: null };
  const document = { documentElement: { dataset: { theme: "light" } } };
  const exports = {};
  // `import("mermaid")` transpiles to a require of a module factory; supply one
  // that fails on the first attempt and succeeds on the second.
  const factory = () => {
    attempts += 1;
    if (attempts === 1) throw new Error("chunk load failed");
    return { default: engine };
  };
  new Function("require", "exports", "document", "getComputedStyle", code)(
    (name) => (name === "mermaid" ? factory() : require(name)),
    exports,
    document,
    () => ({ fontFamily: "Inter, sans-serif" }),
  );

  const first = await exports.renderMermaid("flowchart TD\n  A-->B");
  assert.equal(first.ok, false, "the first attempt should fail");
  assert.match(first.error, /chunk load failed/);

  const second = await exports.renderMermaid("flowchart TD\n  A-->B");
  assert.equal(second.ok, true, `the retry should succeed, got: ${second.error}`);
  assert.equal(attempts, 2, "a failed import must not be cached");
});

// A .mmd file's Source view goes through detectSyntaxLanguageFromFilePath, which
// hands refractor the file extension. The grammar is registered as "mermaid", so
// without an alias the extension asks for a language that does not exist and the
// source renders unhighlighted.
test("a .mmd file resolves to the registered mermaid grammar", async () => {
  const { detectSyntaxLanguageFromFilePath, resolveSyntaxLanguage } = await import(
    "../src/syntaxLanguage.ts"
  );
  for (const path of ["figs/pipeline.mmd", "PLAN.MMD", "notes/diagram.mermaid"]) {
    assert.equal(
      detectSyntaxLanguageFromFilePath(path),
      "mermaid",
      `${path} must resolve to the mermaid grammar, not its raw extension`,
    );
  }
  assert.equal(resolveSyntaxLanguage("mermaid"), "mermaid");
});

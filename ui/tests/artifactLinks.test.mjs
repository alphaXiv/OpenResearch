import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { resolveMarkdownTarget } from "../src/markdownTarget.ts";

const source = readFileSync(new URL("../src/components/ArtifactsTab.tsx", import.meta.url), "utf8");
const tree = ts.createSourceFile("ArtifactsTab.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let fn;
(function visit(node) {
  if (ts.isFunctionDeclaration(node) && node.name?.text === "artifactTarget") fn = node.getText(tree);
  ts.forEachChild(node, visit);
})(tree);
assert.ok(fn);
const compiled = ts.transpileModule(`${fn}\nreturn artifactTarget;`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
const artifactUrl = (projectId, path) => `/api/projects/${projectId}/files/file?path=${encodeURIComponent(path)}`;
const artifactTarget = new Function("resolveMarkdownTarget", "artifactUrl", compiled)(resolveMarkdownTarget, artifactUrl);
const processor = unified().use(remarkParse).use(remarkRehype);
const href = (markdown) => processor.runSync(processor.parse(markdown)).children[0].children[0].properties.href;

test("artifact links to non-ASCII filenames are encoded once", () => {
  assert.deepEqual(artifactTarget("p", "notes", href("[data](数据.md#top)")), {
    path: "notes/数据.md",
    url: "/api/projects/p/files/file?path=notes%2F%E6%95%B0%E6%8D%AE.md#top",
  });
  assert.equal(artifactTarget("p", "", href("[ascii](a.md?path=x&raw=1)"))?.url, "/api/projects/p/files/file?path=a.md&raw=1");
  assert.deepEqual(artifactTarget("p", "", href("[sale](50%off.txt?raw=1)")), {
    path: "50%off.txt",
    url: "/api/projects/p/files/file?path=50%25off.txt&raw=1",
  });
  assert.equal(
    artifactTarget("p", "", href("[section](50%off.html#%E6%95%B0)"))?.url,
    "/api/projects/p/files/file?path=50%25off.html#%E6%95%B0",
  );
  assert.equal(artifactTarget("p", "notes", "../../x.md"), null);
  assert.equal(artifactTarget("p", "notes", "../../50%off.md"), null);
});

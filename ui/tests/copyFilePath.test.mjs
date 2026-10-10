import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import ts from "typescript";

const require = createRequire(import.meta.url);
function copier(clipboard, ClipboardItem) {
  const source = readFileSync(new URL("../src/components/FileTreeActions.tsx", import.meta.url), "utf8");
  const code = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  const exports = {};
  new Function("require", "exports", "navigator", "ClipboardItem", code)(id => {
    if (id === "./ui") return { showAlert: () => {} };
    if (id === "../i18n") return {};
    if (id === "../paraglide/messages.js") return { m: new Proxy({}, { get: (_, name) => () => String(name) }) };
    return require(id);
  }, exports, { clipboard }, ClipboardItem);
  return exports.copyAbsoluteFilePath;
}

test("an asynchronous path starts its clipboard write during the click, before resolution", async () => {
  let resolvePath, item;
  const path = new Promise(resolve => { resolvePath = resolve; });
  const copy = copier({ write: async items => { item = items[0]; } }, class {
    constructor(data) { this.data = data; }
  });
  copy(path);
  assert.ok(item, "write must start before the server answers");
  resolvePath("/actual/canonical/a b/report.md");
  assert.equal(await (await item.data["text/plain"]).text(), "/actual/canonical/a b/report.md");
});

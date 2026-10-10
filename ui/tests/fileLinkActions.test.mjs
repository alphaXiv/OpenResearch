import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import ts from "typescript";
import { loadedFileLocation } from "../src/fileLocation.ts";

const require = createRequire(import.meta.url);

function renderMenu({ loaded, request, remote = false, currentScope = () => 1 }) {
  const calls = [];
  const source = readFileSync(new URL("../src/components/FileLinkActions.tsx", import.meta.url), "utf8");
  const code = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  const mocks = {
    "@tanstack/react-query": { useQuery: options => options.kind === "file"
      ? { data: loaded, isFetching: false, isError: false }
      : { data: { absolutePath: "/exact/resolved/path" }, isFetching: false, isError: false } },
    "../api": { revealFileInManager: async (...args) => { calls.push(["reveal", ...args]); } },
    "../fileLocation": { loadedFileLocation },
    "../queries/files": {
      resolvedFileQuery: (...args) => { calls.push(["read", ...args]); return { kind: "file" }; },
      getFileLocationQuery: (...args) => { calls.push(["location", ...args]); return { kind: "location" }; },
    },
    "../queries/client": { workspaceScope: currentScope, isCurrentScope: scope => scope === currentScope() },
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, key) => () => String(key) }) },
    "../i18n": { ltr: text => text },
    "./ui": { showAlert: (...args) => calls.push(["alert", ...args]) },
    "./FileTreeActions": { FileContextMenu: "menu", copyAbsoluteFilePath: path => calls.push(["copy", path]) },
  };
  const exports = {};
  new Function("require", "exports", code)(id => mocks[id] ?? require(id), exports);
  const tree = exports.FileLinkMenu({ target: { path: request.path, x: 10, y: 10 }, context: { projectId: "project", remote }, request, onClose: () => {}, onOpen: () => {} });
  return { props: tree.props, calls };
}

const checkout = (root, path = "report.md") => ({ source: "checkout", file: { root, path, notFound: false } });

test("menu OS actions use the actual fallback checkout path and session, not the chip path", () => {
  const { props, calls } = renderMenu({ loaded: checkout("worktree", "artifacts/report.md"), request: { source: "artifacts", path: "report.md", sessionId: "chat" } });
  props.onCopyPath();
  props.onReveal();
  assert.deepEqual(calls.find(c => c[0] === "copy"), ["copy", "/exact/resolved/path"]);
  assert.deepEqual(calls.find(c => c[0] === "location"), ["location", "project", { source: "repo", path: "artifacts/report.md", sessionId: "chat", ref: undefined }]);
  assert.deepEqual(calls.find(c => c[0] === "reveal"), ["reveal", "project", "artifacts/report.md", { source: "repo", path: "artifacts/report.md", sessionId: "chat", ref: undefined }]);
});

test("remote links retain copy and panel open but never expose a local reveal action", () => {
  const { props, calls } = renderMenu({ loaded: checkout("clone"), request: { source: "repo", path: "report.md" }, remote: true });
  props.onCopyPath();
  assert.deepEqual(calls.find(c => c[0] === "copy"), ["copy", "/exact/resolved/path"]);
  assert.equal(typeof props.onOpen, "function");
  assert.equal(props.onReveal, undefined);
});

test("switching workspaces makes captured actions inert", () => {
  let scope = 1;
  const { props, calls } = renderMenu({ loaded: checkout("clone"), request: { source: "repo", path: "report.md" }, currentScope: () => scope });
  scope = 2;
  props.onCopyPath();
  props.onReveal();
  assert.equal(calls.some(c => c[0] === "copy" || c[0] === "reveal"), false);
});

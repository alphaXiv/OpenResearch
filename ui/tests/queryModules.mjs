import { readFileSync } from "node:fs";
import ts from "typescript";
import * as query from "@tanstack/react-query";
import * as react from "react";

export function queryModules(api, { nativeClient = false } = {}) {
  const modules = new Map();
  const deletedSessionIds = new Set();
  let client = new query.QueryClient({ defaultOptions: { queries: { staleTime: 30_000, gcTime: Infinity, retry: false } } });
  function load(name) {
    if (name === "client" && !nativeClient) return { queryClient: client, workspaceScope: () => ["workspace", 0], workspaceKey: (family, ...args) => ["workspace", 0, family, ...args], isCurrentScope: () => true, deletedSessionIds };
    if (modules.has(name)) return modules.get(name);
    const source = readFileSync(new URL(`../src/queries/${name}.ts`, import.meta.url), "utf8");
    const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
    const exports = {};
    modules.set(name, exports);
    new Function("require", "exports", code)((id) => {
      if (id === "@tanstack/react-query") return query;
      if (id === "react") return react;
      if (id === "../api") return api;
      if (id.startsWith("./")) return load(id.slice(2));
      throw new Error(`Unexpected query dependency: ${id}`);
    }, exports);
    return exports;
  }
  if (nativeClient) { client.clear(); client = load("client").queryClient; }
  return { load, client };
}

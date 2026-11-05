import { readFileSync } from "node:fs";
import ts from "typescript";
import * as query from "@tanstack/react-query";
import * as react from "react";

// Dependency-free src modules the query layer imports; loaded as they are.
const PURE_MODULES = new Set(["engineHosts", "logWindow"]);

function loadPure(name) {
  const source = readFileSync(new URL(`../src/${name}.ts`, import.meta.url), "utf8");
  const exports = {};
  new Function("exports", ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText)(exports);
  return exports;
}

export function queryModules(api, { nativeClient = false, onChatEvent = () => () => {}, identity = 0, basePath = "" } = {}) {
  const modules = new Map();
  let workspace;
  let client;
  function load(name) {
    if (name === "client" && workspace) return workspace;
    if (modules.has(name)) return modules.get(name);
    const source = readFileSync(new URL(name === "api" ? "../src/api.ts" : `../src/queries/${name}.ts`, import.meta.url), "utf8");
    const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
    const exports = {};
    modules.set(name, exports);
    new Function("require", "exports", code)((id) => {
      if (id === "@tanstack/react-query") return query;
      if (id === "react") return react;
      if (id === "../api") return api;
      if (id === "./i18n") return { fmtNumber: String };
      if (id === "./paraglide/runtime.js") return { getLocale: () => "en" };
      if (id === "./paraglide/messages.js") return { m: new Proxy({}, { get: (_, name) => () => String(name) }) };
      if (id === "../events") return { onChatEvent };
      if (id.startsWith("./") && PURE_MODULES.has(id.slice(2))) return loadPure(id.slice(2));
      if (id.startsWith("./")) return load(id.slice(2));
      if (id.startsWith("../") && PURE_MODULES.has(id.slice(3))) return loadPure(id.slice(3));
      throw new Error(`Unexpected query dependency: ${id}`);
    }, exports);
    const factory = Object.values(exports).find((value) => typeof value === "function" && /^create.*(Queries|Store|Invalidation)$/.test(value.name));
    if (factory) {
      const result = factory(workspace, name === "chatStore" ? { ...load("chat") } : api, name === "settings" ? load("invalidation") : { onChatEvent }, { onEngineHost: () => () => {} });
      Object.assign(exports, result);
    }
    return exports;
  }
  workspace = load("client").createWorkspaceClient(identity, basePath);
  modules.set("client", workspace);
  client = workspace.queryClient;
  if (!nativeClient) client.setDefaultOptions({ queries: { staleTime: 30_000, gcTime: Infinity, retry: false } });
  api ??= load("api").createApi(workspace, load("invalidation"));
  return { load, client, api, workspace };
}

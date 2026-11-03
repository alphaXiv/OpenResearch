import { mockWorkspace } from "./workspaceMocks.mjs";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const require = createRequire(import.meta.url);
const src = fileURLToPath(new URL("../src/", import.meta.url));

export function runLabelsHarness(locale = "en", sessionExperiments = []) {
  const catalog = JSON.parse(readFileSync(new URL(`../messages/${locale}.json`, import.meta.url), "utf8"));
  const modules = new Map();
  const mocks = {
    "paraglide/messages.js": { m: new Proxy({}, { get: (_, key) => (inputs = {}) => catalog[key].replace(/\{(\w+)\}/g, (_, name) => String(inputs[name])) }) },
    "paraglide/runtime.js": { getLocale: () => locale },
    "api": { fmtDuration: () => "1s", fmtNumber: String, timeAgo: () => "1d", cleanupRun: async () => {}, cancelRun: async () => {}, retryEngineHost: async () => ({}) },
    "@tanstack/react-query": { useQuery: () => ({ data: sessionExperiments, isPending: false, error: null }) },
    "react-dom": { createPortal: (children) => children },
    "queries/chat": { getSessionExperimentsQuery: () => ({}) },
    "components/ModelPicker": { harnessLabel: () => "Codex" },
    "components/HarnessLogo": { HarnessLogo: () => null },
    "components/tourGeometry": { useMeasure: () => ({ ref: () => {}, offsetHeight: 300 }), usePopoverPosition: () => ({ x: 8, y: 8 }) },
    "workspace": { useWorkspace: () => mockWorkspace({ ...mocks.api, ...mocks.dialog, ...mocks["queries/settings"], getSessionExperimentsQuery: () => ({}), requestHostConnect: () => {} }) },
    "queries/settings": { useEngineHosts: () => [], setEngineHost: () => {} },
    "components/AbandonRunDialog": { AbandonRunDialog: () => null },
  };
  function load(name) {
    if (mocks[name]) return mocks[name];
    if (modules.has(name)) return modules.get(name);
    if (name === "components/ui") return { ...load("components/ui/Button"), ...load("components/ui/StatusIndicator"), ...load("components/ui/Badge") };
    const path = resolve(src, name);
    const file = existsSync(path + ".ts") ? path + ".ts" : path + ".tsx";
    const code = ts.transpileModule(readFileSync(file, "utf8"), {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
      fileName: file,
    }).outputText;
    const exports = {};
    modules.set(name, exports);
    new Function("require", "exports", code)((id) => {
      if (!id.startsWith(".")) return mocks[id] ?? require(id);
      const dependency = resolve(dirname(file), id).slice(src.length);
      return load(dependency);
    }, exports);
    return exports;
  }
  return { load };
}

import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import React from "react";
import ts from "typescript";

const require = createRequire(import.meta.url);

export function dialogHarness() {
  const state = [];
  let cursor = 0;
  let mounted = false;
  const exports = {};
  const compiled = ts.transpileModule(readFileSync(new URL("../src/dialog.tsx", import.meta.url), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  new Function("require", "exports", "requestAnimationFrame", compiled)((name) => {
    if (name === "react") return {
      ...React,
      useState(initial) {
        const index = cursor++;
        if (!(index in state)) state[index] = initial;
        return [state[index], (next) => { state[index] = typeof next === "function" ? next(state[index]) : next; }];
      },
      useRef(initial) {
        const index = cursor++;
        state[index] ??= { current: initial };
        return state[index];
      },
      useEffect(effect) { if (!mounted) effect(); },
    };
    if (name === "./components/ui") return { Dialog: "dialog", DialogContent: "content", DialogTitle: "title", DialogDescription: "description", DialogHeader: "header", DialogFooter: "footer", Button: "button", Spinner: "spinner" };
    if (name === "./paraglide/messages.js") return { m: new Proxy({}, { get: (_, key) => () => key }) };
    return require(name);
  }, exports, (callback) => callback());

  function render() {
    cursor = 0;
    const roots = exports.DialogManager();
    mounted = true;
    return roots;
  }
  render();
  return { ...exports, render };
}

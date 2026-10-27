import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import ts from "typescript";

const require = createRequire(import.meta.url);
const code = ts.transpileModule(readFileSync(new URL("../src/components/SshConnectTerminal.tsx", import.meta.url), "utf8"), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX },
}).outputText;

test("Connect shows the server diagnostic even after an authentication prompt", () => {
  let socket;
  const lines = [];
  const terminal = {
    options: {}, focus() {}, blur() {},
    onData: () => ({ dispose() {} }), onResize: () => ({ dispose() {} }),
    write() {}, writeln: (line) => lines.push(line),
  };
  const mocks = {
    react: {
      useRef: (value) => ({ current: value ?? {} }),
      useState: (value) => [value, () => {}],
      useEffect: (effect) => effect(),
    },
    "../paraglide/messages.js": { m: {} },
    "../i18n": {},
    "../workspaceState": { isRecord: (value) => typeof value === "object" && value !== null },
    "./ui/Spinner": {},
    "./terminal": { mountTerminal: () => ({ terminal, dispose() {} }) },
  };
  class Socket {
    constructor() { socket = this; }
    close() {}
  }
  const exports = {};
  new Function("require", "exports", "WebSocket", "location", code)(
    (name) => mocks[name] ?? require(name), exports, Socket, { protocol: "http:", host: "localhost" },
  );
  exports.CommandTerminal({ path: "/connect", label: "Connect", onComplete: () => false });
  socket.onmessage({ data: new TextEncoder().encode("Password: ").buffer });
  const error = "SSH helper did not start. Python 3.8+ is required.";
  socket.onmessage({ data: JSON.stringify({ type: "error", error }) });
  assert.deepEqual(lines, [`\r\n${error}`]);
  assert.equal(terminal.options.disableStdin, true);
});

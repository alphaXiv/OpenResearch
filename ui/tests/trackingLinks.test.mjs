import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";

const require = createRequire(import.meta.url);
const source = readFileSync(new URL("../src/components/TrackingLinks.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX },
}).outputText;

const command = "tensorboard --logdir '/data/tensorboard/demo'";
const run = {
  backend: {
    kind: "local_job",
    tracking: [
      { kind: "tensorboard", log_dir: "/data/tensorboard/demo/run-1", root_log_dir: "/data/tensorboard/demo", run: "run-1" },
    ],
  },
};

// Load the component against a given `navigator`, and return its copy button's
// click handler plus every alert the click raised.
function copyButton(navigator) {
  const alerts = [];
  let onClick;
  const mocks = {
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
    "../i18n": { ltr: (value) => value },
    "../api": { runTracking: (value) => value.backend.tracking },
    "./ui": {
      IconButton: (props) => {
        onClick = props.onClick;
        return null;
      },
      IconButtonLink: () => null,
      showAlert: (...args) => alerts.push(args),
    },
    "lucide-react": { Copy: () => null, LineChart: () => null },
  };
  const exports = {};
  new Function("require", "exports", "navigator", compiled)((name) => mocks[name] ?? require(name), exports, navigator);
  renderToStaticMarkup(React.createElement(exports.TrackingLinks, { run }));
  return {
    alerts,
    click: async () => {
      onClick({ stopPropagation() {} });
      for (let turn = 0; turn < 5; turn++) await Promise.resolve();
    },
  };
}

function shownCommand(alerts) {
  return alerts.map(([message, type, options]) => [message, type, options && renderToStaticMarkup(options.description)]);
}

test("a refused clipboard write shows the TensorBoard command as selectable text", async () => {
  const button = copyButton({ clipboard: { writeText: async () => { throw new Error("Document is not focused."); } } });
  await button.click();
  assert.deepEqual(shownCommand(button.alerts), [[
    "tracking_links_copy_tensorboard_failed",
    "error",
    `<code dir="ltr" class="select-all font-mono text-sm">${command.replaceAll("'", "&#x27;")}</code>`,
  ]]);
});

test("a missing clipboard shows the TensorBoard command as selectable text", async () => {
  const button = copyButton({});
  await button.click();
  assert.deepEqual(shownCommand(button.alerts), [[
    "tracking_links_copy_tensorboard_failed",
    "error",
    `<code dir="ltr" class="select-all font-mono text-sm">${command.replaceAll("'", "&#x27;")}</code>`,
  ]]);
});

test("a successful copy writes the recorded command and confirms it", async () => {
  const written = [];
  const button = copyButton({ clipboard: { writeText: async (text) => { written.push(text); } } });
  await button.click();
  assert.deepEqual(written, [command]);
  assert.deepEqual(button.alerts, [["common_copied", "success"]]);
});

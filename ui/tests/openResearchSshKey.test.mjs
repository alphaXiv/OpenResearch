import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import ts from "typescript";

const require = createRequire(import.meta.url);
const code = ts.transpileModule(readFileSync(new URL("../src/components/OpenResearchSshKey.tsx", import.meta.url), "utf8"), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;

function render(sshKey, mutation = {}) {
  const alerts = [];
  const adds = [];
  const add = { isPending: false, isError: false, error: null, mutate: (_, options) => adds.push(options), ...mutation };
  const mocks = {
    "lucide-react": { KeyRound: "key-icon" },
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
    "../queries/settings": { useAddOpenResearchSshKey: () => add },
    "./ui": { Button: "button", Spinner: "spinner", showAlert: (message, type) => alerts.push([message, type]) },
  };
  const exports = {};
  new Function("require", "exports", code)((name) => mocks[name] ?? require(name), exports);
  const nodes = (tree) => React.isValidElement(tree) ? [tree, ...React.Children.toArray(tree.props.children).flatMap(nodes)] : [];
  const tree = nodes(exports.OpenResearchSshKey({ sshKey }));
  const text = tree.flatMap((node) => React.Children.toArray(node.props.children)).filter((child) => typeof child === "string");
  return { tree, text, button: tree.find((node) => node.type === "button"), alerts, adds };
}

const existing = { path: "~/.ssh/id_ed25519.pub", fingerprint: "SHA256:pU0aHZujtAnVqRHnpcOVxPe07Yft4bANE4UgHDCaYL4" };

test("names the exact key and what uploading it does, with one primary button", () => {
  const step = render(existing);
  assert.ok(step.text.includes(existing.path));
  assert.ok(step.text.includes(existing.fingerprint));
  assert.ok(step.text.includes("settings_ssh_key_upload_note"));
  assert.ok(!step.text.includes("settings_ssh_key_new"));
  assert.equal(step.button.props.variant, "primary");
  assert.ok(step.text.includes("settings_ssh_key_add"));
});

test("a missing key says adding creates it", () => {
  const step = render({ path: existing.path, fingerprint: null });
  assert.ok(step.text.includes(existing.path));
  assert.ok(step.text.includes("settings_ssh_key_new"));
});

test("adding uploads once per click and confirms with a toast", () => {
  const step = render(existing);
  step.button.props.onClick();
  assert.equal(step.adds.length, 1);
  step.adds[0].onSuccess();
  assert.deepEqual(step.alerts, [["settings_ssh_key_added", "success"]]);
  const pending = render(existing, { isPending: true });
  assert.equal(pending.button.props.disabled, true);
  assert.ok(pending.text.includes("settings_ssh_key_adding"));
});

test("a failure shows its reason and offers a retry", () => {
  const step = render(existing, { isError: true, error: new Error("SSH key setup timed out. Try again.") });
  const alert = step.tree.find((node) => node.props.role === "alert");
  assert.equal(alert.props.children, "SSH key setup timed out. Try again.");
  assert.ok(step.text.includes("app_retry"));
  assert.equal(step.button.props.disabled, false);
});

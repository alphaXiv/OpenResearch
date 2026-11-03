import { withWorkspaceMocks } from "./workspaceMocks.mjs";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";
import { runLabelsHarness } from "./runLabelsHarness.mjs";

const require = createRequire(import.meta.url);
const en = JSON.parse(readFileSync(new URL("../messages/en.json", import.meta.url), "utf8"));
// English catalogue text with its placeholders filled, so assertions read as the UI does.
const messages = {
  m: new Proxy({}, {
    get: (_, key) => (inputs = {}) => {
      assert.ok(Object.hasOwn(en, key), `en.json has no message ${String(key)}`);
      return en[key].replace(/\{(\w+)\}/g, (_, name) => String(inputs[name]));
    },
  }),
};

function transpile(file) {
  return ts.transpileModule(readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
}

const runDisplay = {};
new Function("exports", transpile("runDisplay.ts"))(runDisplay);
const engineHosts = {};
new Function("exports", transpile("engineHosts.ts"))(engineHosts);

let hosts = [];
const exports = {};
const mocks = {
  "lucide-react": { CircleStop: () => null, Plug: () => null, RefreshCw: () => null },
  "../api": { cleanupRun: async () => {}, cancelRun: async () => {}, retryEngineHost: async () => ({}) },
  "../engineHosts": engineHosts,
  "../events": { requestHostConnect: () => {} },
  "../i18n": { fmtClock: (ms) => `T${ms}`, ltr: (text) => text },
  "../paraglide/messages.js": messages,
  "../queries/settings": { useEngineHosts: () => hosts, setEngineHost: () => {} },
  "../runDisplay": runDisplay,
  "./AbandonRunDialog": { AbandonRunDialog: () => null },
  "./ui": { Button: ({ children, variant }) => React.createElement("button", { "data-variant": variant }, children) },
  "./StatusBadge": runLabelsHarness().load("components/StatusBadge"),
  "./ui/cn": { cn: (...values) => values.filter(Boolean).join(" ") },
};
new Function("require", "exports", transpile("components/RunContactNotice.tsx"))(withWorkspaceMocks((name) => mocks[name] ?? require(name)), exports);

const unreachable = { type: "unreachable", since: 100, cause: "Operation timed out" };
const needsAuth = { type: "needs_auth", since: 200, cause: "Duo approval required" };
const unreconciled = (detail) => ({ type: "unreconciled", since: 300, detail });

function render(extra) {
  const run = { id: "run", status: "running", cancelling: false, contact: null, lostReason: null, host: "gpu-box", backend: null, ...extra };
  return renderToStaticMarkup(React.createElement(exports.RunContactNotice, { run }));
}
const buttons = (html) => [...html.matchAll(/<button[^>]*>(.*?)<\/button>/g)].map((match) => match[1]);

test("a live run with nothing to add renders nothing", () => {
  assert.equal(render({}), "");
  assert.equal(render({ status: "done" }), "");
});

test("a legacy monitoring error renders as before, without actions", () => {
  const html = render({ backend: { monitoringError: "Monitoring unavailable: no exit status." } });
  assert.match(html, /role="status"/);
  assert.match(html, /text-accent-amber[^>]*>Monitoring unavailable: no exit status\.</);
  assert.deepEqual(buttons(html), []);
});

test("unreachable names the host, its next try when backing off, and offers Retry for key hosts", () => {
  hosts = [{ alias: "gpu-box", status: "backoff", class: "key", sinceMs: 100, nextRetryMs: 900, root: "/runs", cause: null }];
  try {
    const html = render({ contact: unreachable });
    assert.match(html, />Can&#x27;t reach gpu-box since T100\. Trying again at T900\.</);
    assert.match(html, />Operation timed out</);
    assert.deepEqual(buttons(html), ["Retry", "Abandon…"]);
  } finally {
    hosts = [];
  }
  assert.deepEqual(buttons(render({ contact: unreachable })), ["Abandon…"]);
  for (const status of ["live", "connecting", "needs_auth"]) {
    hosts = [{ alias: "gpu-box", status, class: "key", sinceMs: 100, nextRetryMs: null, root: "/runs", cause: null }];
    assert.deepEqual(buttons(render({ contact: unreachable })), ["Abandon…"], `no Retry while ${status}`);
  }
  hosts = [];
  assert.match(render({ contact: unreachable, host: null }), />Can&#x27;t reach the host since T100\.</);
});

test("needs auth offers Connect first", () => {
  const html = render({ contact: needsAuth });
  assert.match(html, />Connect to gpu-box to keep following this run\.</);
  assert.match(html, /<button data-variant="primary">Connect<\/button>/);
  assert.deepEqual(buttons(html), ["Connect", "Abandon…"]);
});

test("pending and unverified cancels say why", () => {
  assert.match(render({ cancelling: true, contact: unreachable }), />Cancelling…<.*>Cancel pending: gpu-box unreachable since T100\. orx sends it when the host is back\.</);
  assert.deepEqual(buttons(render({ cancelling: true, contact: needsAuth })), ["Connect", "Abandon…"]);
  assert.match(render({ status: "submitting", cancelling: true, contact: unreconciled("No sandbox id") }), />Cancelling…<.*>Cancel pending: the launch isn&#x27;t confirmed yet\. If the job turns up, orx stops it\.</);

  const unverified = render({ cancelling: true, contact: unreconciled("2 processes still running") });
  assert.match(unverified, />Cancelling…<.*>2 processes still running</);
  assert.deepEqual(buttons(unverified), ["Stop again", "Abandon…"]);
});

test("an unconfirmed launch can only be abandoned", () => {
  const html = render({ status: "submitting", contact: unreconciled("The app stopped before Modal returned a sandbox id.") });
  assert.match(html, /text-accent-orange[^>]*>Couldn&#x27;t confirm the launch\. Checking again\.</);
  assert.deepEqual(buttons(html), ["Abandon…"]);
});

test("an abandoned run's pending kill is stated quietly, with no actions", () => {
  const html = render({ status: "lost", lostReason: "abandoned", contact: unreconciled("Abandoned") });
  assert.match(html, /text-subtext[^>]*>Abandoned\. If the job turns up on gpu-box, orx stops it\.</);
  assert.deepEqual(buttons(html), []);
});

test("a finished run's log collection is stated, with the cause when it failed", () => {
  const pending = render({ status: "done", logCollection: "pending" });
  assert.match(pending, /text-subtext[^>]*>Saving the log from gpu-box…</);
  assert.deepEqual(buttons(pending), []);

  const failed = render({ status: "done", logCollection: "failed", logCollectionError: "No space left on device" });
  assert.match(failed, /text-accent-amber[^>]*>Couldn&#x27;t save the log from gpu-box\. orx keeps trying; the log still opens from the host\.<.*>No space left on device</);
});

test("a finished run whose result never reached the chat says why, in one line", () => {
  const html = render({ status: "done", noticeError: "Chat harness is no longer configured" });
  assert.match(html, /text-accent-amber[^>]*>Couldn&#x27;t post this run&#x27;s result to the chat: Chat harness is no longer configured<\/p><\/div>/);
  assert.deepEqual(buttons(html), []);
});

test("Retry follows the connection of the run's own root", () => {
  hosts = [
    { alias: "gpu-box", root: "/a", status: "backoff", class: "key", sinceMs: 100, nextRetryMs: null, cause: null },
    { alias: "gpu-box", root: "/b", status: "live", class: "key", sinceMs: 100, nextRetryMs: null, cause: null },
  ];
  try {
    const on = (root) => render({ contact: unreachable, backend: { kind: "engine", handle: { type: "Ssh", host: "gpu-box", root } } });
    assert.deepEqual(buttons(on("/a")), ["Retry", "Abandon…"]);
    assert.deepEqual(buttons(on("/b")), ["Abandon…"]);
  } finally {
    hosts = [];
  }
});

test("the message spans the strip and actions sit on their own row beneath it", () => {
  const html = render({ contact: needsAuth });
  assert.match(html, /^<div class="run-contact-notice flex flex-col[^"]*">/);
  const message = html.indexOf('role="status"');
  const actions = html.indexOf('class="run-contact-actions');
  assert.ok(message !== -1 && actions > message, "actions follow the message");
  assert.doesNotMatch(html.slice(message, actions), /<button/);
  assert.doesNotMatch(render({ status: "done", logCollection: "pending" }), /run-contact-actions/);
});

test("a cancel waiting on a backing-off host offers Retry beside Abandon", () => {
  hosts = [{ alias: "gpu-box", root: "/runs", status: "backoff", class: "key", sinceMs: 100, nextRetryMs: null, cause: null }];
  try {
    assert.deepEqual(buttons(render({ cancelling: true, contact: unreachable })), ["Retry", "Abandon…"]);
  } finally {
    hosts = [];
  }
});

test("a cancelled run with survivors names the host and offers Cleanup as Stop now", () => {
  const detail = "Not fully stopped · 2 processes still running on gpu-box";
  const html = render({ status: "cancelled", contact: { type: "stopping", since: 300, count: 2 } });
  assert.match(html, /role="status"/);
  assert.ok(html.replace(/[\u2066-\u2069]/g, "").includes(detail));
  assert.doesNotMatch(html, /keeps stopping/);
  assert.deepEqual(buttons(html), ["Stop now"]);
  assert.equal(render({ status: "cancelled", contact: null }), "");
});

test("a cancelled tombstone retains its survivor proof while host authentication needs attention", () => {
  const detail = "Not fully stopped · 2 processes still running on gpu-box";
  const run = { status: "cancelled", contact: { type: "stopping", since: 300, count: 2 } };
  hosts = [{ alias: "gpu-box", root: "/runs", status: "needs_auth", class: "interactive", sinceMs: 100, nextRetryMs: null, cause: "MFA" }];
  try {
    const html = render(run);
    assert.ok(html.replace(/[\u2066-\u2069]/g, "").includes(detail));
    assert.deepEqual(buttons(html), ["Connect", "Stop now"]);
  } finally { hosts = []; }
});

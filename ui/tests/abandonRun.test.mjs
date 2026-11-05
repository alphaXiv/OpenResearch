import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { dialogHarness } from "./dialogHarness.mjs";
import { runLabelsHarness } from "./runLabelsHarness.mjs";

function openAbandon(locale = "en", contact = { type: "unreachable", since: 1, cause: "Offline" }, abandon = async () => {}) {
  const dialogs = dialogHarness();
  const { load } = runLabelsHarness(locale, [], {
    react: { ...React, useState: (initial) => [initial, () => {}] },
    dialog: dialogs,
    api: { abandonRun: abandon },
  });
  const { RunContactNotice } = load("components/RunContactNotice");
  const notice = RunContactNotice({ run: { id: "run", status: "running", cancelling: true, contact, host: "gpu-box", backend: null } });
  const actions = notice.props.children[1];
  const button = actions.props.children.find((child) => child && child.props.onClick && child.props.children === load("paraglide/messages.js").m.run_abandon());
  assert.ok(button, "the pending run offers Abandon");
  button.props.onClick();
  return dialogs;
}

const settle = () => new Promise((resolve) => setImmediate(resolve));

test("Abandon uses the shared confirmation and warns about unreachable compute", async () => {
  const calls = [];
  const dialogs = openAbandon("en", undefined, async (id) => { calls.push(id); });
  const [root] = dialogs.render();
  const content = root.props.children;
  const html = renderToStaticMarkup(content);
  assert.match(html, /Abandon this run\?/);
  assert.match(html, /The machine is unreachable\./);
  assert.match(html, /OpenResearch will stop tracking this run\./);
  assert.match(html, /Double-check that the actual compute is shut down\./);
  assert.match(html, /Abandon run/);
  assert.equal(content.props.destructive, true);
  assert.equal(content.props.cancelable, true);
  assert.deepEqual(calls, []);
  content.props.confirm();
  await settle();
  assert.deepEqual(calls, ["run"]);
  assert.equal(dialogs.render()[0].props.open, false);
});

test("dismissing Abandon leaves the run tracked", async () => {
  const dialogs = openAbandon("en", undefined, () => assert.fail("dismissal must not abandon"));
  dialogs.render()[0].props.children.props.cancel();
  await settle();
  assert.equal(dialogs.render()[0].props.open, false);
});

test("failed abandonment keeps the shared dialog open with the error", async () => {
  const dialogs = openAbandon("en", undefined, async () => { throw new Error("Host changed"); });
  dialogs.render()[0].props.children.props.confirm();
  await settle();
  const [root] = dialogs.render();
  assert.equal(root.props.open, true);
  assert.equal(root.props.children.props.isDisabled, false);
  assert.equal(root.props.children.props.errorMessage, "Host changed");
});

test("Japanese Abandon translates the warning; auth does not claim the machine is unreachable", () => {
  const dialogs = openAbandon("ja");
  const html = renderToStaticMarkup(dialogs.render()[0].props.children);
  assert.ok(html.includes("マシンに接続できません。"), html);
  assert.ok(html.includes("実際の計算リソースが停止していることを再確認してください。"), html);
  const auth = openAbandon("en", { type: "needs_auth", since: 1, cause: "Connect" });
  const authHtml = renderToStaticMarkup(auth.render()[0].props.children);
  assert.doesNotMatch(authHtml, /The machine is unreachable/);
  assert.match(authHtml, /Double-check that the actual compute is shut down/);
});

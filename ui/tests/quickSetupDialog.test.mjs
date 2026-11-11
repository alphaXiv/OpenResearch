import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { TARGET_LABELS } from "../src/computeTargets.ts";
import { m } from "../src/paraglide/messages.js";
import { find, press, renderApp, serveApp, settle, type } from "./render.mjs";

const k8s = {
  configured: false, contexts: [], currentContext: null, context: null, namespace: "default",
  preflight: { kubectlFound: true, reachable: true, canCreateJobs: true, canPatchJobs: true },
};

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};

/** Opens Kubernetes Quick Setup from the compute page; saving its settings waits for `save`. */
async function quickSetup(save = () => k8s) {
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }),
    "GET /api/settings/compute": () => ({ defaultBackend: null, defaultFlavor: null, targets: [{ id: "k8s", configured: false, enabled: true, summary: "" }] }),
    "GET /api/settings/k8s": () => k8s,
    "POST /api/settings/k8s": save,
  });
  await renderApp("/projects/p/settings/compute");
  const dialogs = () => document.querySelectorAll('[role="dialog"]').length;
  const open = async () => {
    await click(document.querySelector(`button[aria-label="${TARGET_LABELS.k8s()}"]`));
    assert.equal(dialogs(), 1);
  };
  const dismiss = async () => {
    await press("Escape");
    await settle();
  };
  const edit = () => type(document.querySelector(`[role="dialog"] input[placeholder="${m.settings_page_default()}"]`), "research");
  const discard = () => document.body.textContent.includes(m.settings_k8s_discard_changes());
  return { dialogs, open, dismiss, edit, discard };
}

test("Quick Setup closes clean edits and blocks dismissal while saving", async () => {
  const setup = await quickSetup(() => new Promise(() => {}));
  await setup.open();
  await setup.dismiss();
  assert.equal(setup.dialogs(), 0);

  await setup.open();
  await setup.edit();
  await click(find('[role="dialog"] button', m.common_save()));
  assert.ok(find('[role="dialog"] button', m.common_saving()));
  await setup.dismiss();
  assert.equal(setup.dialogs(), 1);
  assert.equal(setup.discard(), false);
});

test("Quick Setup awaits discard consent and keeps dirty edits when consent is cancelled", async () => {
  const setup = await quickSetup();
  await setup.open();
  await setup.edit();
  await setup.dismiss();
  assert.equal(setup.discard(), true);
  await click(find("button", m.common_cancel()));
  assert.equal(setup.discard(), false);
  assert.equal(setup.dialogs(), 1);
  assert.equal(document.querySelector('[role="dialog"] input').value, "research");

  await setup.dismiss();
  await click(find("button", m.common_confirm()));
  assert.equal(setup.dialogs(), 0);
});

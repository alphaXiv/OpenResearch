import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { HfSection } from "../src/components/SettingsPage.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, render, serveApp, settle } from "./render.mjs";

const valid = {
  configured: true, source: "openresearchEnv", maskedToken: "hf_…abcd", validationStatus: "valid",
  validationError: null, username: "ada", jobsWrite: true, sourceStaging: true, sourceStagingError: null,
};

async function hfSettings(settings) {
  const saved = [];
  serveApp({
    "GET /api/settings/hf": () => settings,
    "POST /api/settings/hf/namespace": (init) => {
      saved.push(JSON.parse(init.body));
      return { ...settings, namespace: "ada", namespaceSource: null, namespaceError: null };
    },
  });
  await render(createElement(HfSection, { remote: false }));
  await settle();
  return saved;
}

test("a server older than the namespace choice still shows the HF account, without a picker", async () => {
  await hfSettings(valid);
  assert.ok(find("dd", "ada"), "the account is shown");
  assert.ok(!find("dt", m.settings_hf_namespace()), "no picker without the server's organizations");
});

test("the picker lists the account and each organization with its plan", async () => {
  await hfSettings({
    ...valid, namespace: "lab", namespaceSource: "syncedEnv", namespaceError: null,
    orgs: [{ name: "lab", plan: "team", canPay: true, role: "admin" }, { name: "club", plan: null, canPay: null, role: null }],
  });
  assert.ok(find("dt", m.settings_hf_namespace()));
  assert.ok(find("button", m.settings_hf_namespace_org_plan({ name: "lab", plan: "team" })), "the chosen organization shows its plan");
});

test("a saved organization the account left can be cleared from the dialog", async () => {
  const saved = await hfSettings({
    ...valid, namespace: null, namespaceSource: "syncedEnv", orgs: [],
    namespaceError: "orx is set to launch Hugging Face Jobs in lab, but the token's account ada isn't lab or a member of it.",
  });
  assert.ok(find("p", "isn't lab or a member of it"), "the error says which organization");
  const reset = find("button", m.settings_hf_namespace_use_personal({ name: "ada" }));
  assert.ok(reset, "a way back to the personal account");
  await act(async () => reset.click());
  await settle();
  assert.deepEqual(saved, [{ namespace: null }]);
});

test("a namespace set in the process environment offers no reset here", async () => {
  await hfSettings({
    ...valid, namespace: null, namespaceSource: "env", orgs: [],
    namespaceError: "orx is set to launch Hugging Face Jobs in lab (ORX_HF_NAMESPACE, from the environment).",
  });
  assert.ok(!find("button", m.settings_hf_namespace_use_personal({ name: "ada" })));
});

import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { HfSection } from "../src/components/SettingsPage.tsx";
import { ltr } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { find, render, serveApp, settle } from "./render.mjs";

const valid = {
  configured: true, source: "openresearchEnv", maskedToken: "hf_…abcd", validationStatus: "valid",
  validationError: null, username: "ada", jobsWrite: true, sourceStaging: true, sourceStagingError: null,
};

const invalidNamespace = {
  ...valid,
  namespace: null,
  jobsWrite: null,
  sourceStaging: null,
};

async function hfSettings(settings, overrides = {}) {
  const saved = [];
  serveApp({
    "GET /api/settings/hf": () => settings,
    "POST /api/settings/hf/namespace": (init) => {
      const body = JSON.parse(init.body);
      saved.push(body);
      return {
        ...valid,
        orgs: settings.orgs ?? [],
        namespace: body.namespace ?? "ada",
        namespaceSource: body.namespace ? "syncedEnv" : null,
        namespaceError: null,
      };
    },
    ...overrides,
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

test("the picker lists the account and each organization with its plan and links its label", async () => {
  await hfSettings({
    ...valid, namespace: "lab", namespaceSource: "syncedEnv", namespaceError: null,
    orgs: [{ name: "lab", plan: "team", canPay: true, role: "admin" }, { name: "club", plan: null, canPay: null, role: null }],
  });
  const label = find("dt", m.settings_hf_namespace());
  assert.ok(label?.id, "the label has an id for aria-labelledby");
  const trigger = find("button", m.settings_hf_namespace_org_plan({ name: ltr("lab"), plan: ltr("team") }));
  assert.ok(trigger, "the chosen organization shows its plan");
  assert.ok(trigger.getAttribute("aria-labelledby")?.includes(label.id), "the trigger is labelled by its dt");
});

test("a saved organization the account left can be cleared from the dialog without a false OAuth warning", async () => {
  const saved = await hfSettings({
    ...invalidNamespace, namespaceSource: "syncedEnv", orgs: [],
    namespaceError: "The token's account ada isn't lab or a member of it, so Hugging Face Jobs can't launch there. Choose one of: ada.",
  });
  assert.ok(find("p", "isn't lab or a member of it"), "the error says which organization");
  assert.ok(!find("p", "OAuth tokens"), "does not show the OAuth warning when jobsWrite is null due to namespaceError");
  const reset = find("button", m.settings_hf_namespace_use_personal({ name: ltr("ada") }));
  assert.ok(reset, "a way back to the personal account");
  await act(async () => reset.click());
  await settle();
  assert.deepEqual(saved, [{ namespace: null }]);
});

test("an invalid saved organization with other organizations shows a placeholder instead of personal", async () => {
  const saved = await hfSettings({
    ...invalidNamespace, namespaceSource: "syncedEnv",
    orgs: [{ name: "pbai", plan: "team", canPay: true, role: "admin" }],
    namespaceError: "The token's account ada isn't lab or a member of it, so Hugging Face Jobs can't launch there. Choose one of: ada, pbai.",
  });
  assert.ok(!find("p", "OAuth tokens"), "does not show the OAuth warning");
  assert.ok(!find("button", m.settings_hf_namespace_personal({ name: ltr("ada") })), "does not pretend personal is selected");
  const trigger = find("button", m.settings_hf_namespace_choose());
  assert.ok(trigger, "shows the choose placeholder");
  await act(async () => trigger.click());
  const orgChoice = find("button", m.settings_hf_namespace_org_plan({ name: ltr("pbai"), plan: ltr("team") }));
  assert.ok(orgChoice, "lists member organizations");
  await act(async () => orgChoice.click());
  await settle();
  assert.deepEqual(saved, [{ namespace: "pbai" }]);
});

test("an invalid namespace in the process environment hides the picker and offers no reset here", async () => {
  await hfSettings({
    ...invalidNamespace, namespaceSource: "env",
    orgs: [{ name: "pbai", plan: "team", canPay: true, role: "admin" }],
    namespaceError: "The token's account ada isn't lab or a member of it, so Hugging Face Jobs can't launch there. Choose one of: ada, pbai.",
  });
  assert.ok(!find("dt", m.settings_hf_namespace()), "hides the picker while env override is invalid");
  assert.ok(!find("button", m.settings_hf_namespace_use_personal({ name: ltr("ada") })));
  assert.ok(!find("p", "OAuth tokens"), "does not show the OAuth warning");
  assert.ok(find("p", m.settings_hf_namespace_env_override()), "explains the env override");
});

test("Check again and namespace saves disable each other while pending", async () => {
  const initial = {
    ...valid, namespace: "lab", namespaceSource: "syncedEnv", namespaceError: null,
    orgs: [{ name: "lab", plan: "team", canPay: true, role: "admin" }],
  };
  let getCount = 0;
  let releaseRefresh;
  let releaseSave;
  await hfSettings(initial, {
    "GET /api/settings/hf": () => {
      getCount += 1;
      if (getCount === 1) return initial;
      return new Promise((resolve) => { releaseRefresh = () => resolve(initial); });
    },
    "POST /api/settings/hf/namespace": () =>
      new Promise((resolve) => {
        releaseSave = () => resolve({ ...initial, namespace: "ada", namespaceSource: null });
      }),
  });

  const checkAgain = find("button", m.settings_check_again());
  const picker = find("button", m.settings_hf_namespace_org_plan({ name: ltr("lab"), plan: ltr("team") }));
  assert.ok(checkAgain && picker);

  await act(async () => checkAgain.click());
  assert.equal(picker.disabled, true, "picker disables while Check again is pending");
  await act(async () => { releaseRefresh(); });
  await settle();
  assert.equal(picker.disabled, false, "picker re-enables after Check again finishes");

  await act(async () => picker.click());
  const personal = find("button", m.settings_hf_namespace_personal({ name: ltr("ada") }));
  assert.ok(personal);
  await act(async () => personal.click());
  assert.equal(checkAgain.disabled, true, "Check again disables while namespace save is pending");
  await act(async () => { releaseSave(); });
  await settle();
  assert.equal(checkAgain.disabled, false, "Check again re-enables after namespace save finishes");
});


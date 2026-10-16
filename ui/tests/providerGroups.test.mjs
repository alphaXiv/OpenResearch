import assert from "node:assert/strict";
import test from "node:test";
import { providerGroups, providerSwitchTarget } from "../src/providerGroups.ts";

const plan = { id: "coding-plan", label: "Coding Plan" };
const key = { id: "api-key", label: "API key" };
const models = [
  { id: "account:zai-individual-coding-plan/GLM-5.3", displayName: "GLM-5.3", providerGroup: plan },
  { id: "account:zai-individual-coding-plan/GLM-5.3-Flash", displayName: "GLM-5.3-Flash", providerGroup: plan },
  { id: "zai-standard-api/GLM-5.3-Flash", displayName: "GLM-5.3-Flash", providerGroup: key },
  { id: "zai-standard-api/GLM-5", displayName: "GLM-5", providerGroup: key },
];
const label = (model) => model.displayName;

test("provider groups come in catalog order, once each", () => {
  assert.deepEqual(providerGroups(models), [{ ...plan, available: true }, { ...key, available: true }]);
  assert.deepEqual(providerGroups(models.map(({ providerGroup, ...rest }) => rest)), []);
});

test("declared provider groups are offered before they have models", () => {
  const planOnly = models.filter((entry) => entry.providerGroup === plan);
  assert.deepEqual(providerGroups(planOnly, [plan, key]), [
    { ...plan, available: true },
    { ...key, available: false },
  ]);
  assert.deepEqual(providerGroups([], [plan, key]).map((group) => group.available), [false, false]);
});

test("switching provider keeps the model when the other provider has it", () => {
  assert.equal(
    providerSwitchTarget(models, "account:zai-individual-coding-plan/GLM-5.3-Flash", "api-key", label),
    "zai-standard-api/GLM-5.3-Flash",
  );
  // Otherwise the provider's first model.
  assert.equal(
    providerSwitchTarget(models, "zai-standard-api/GLM-5", "coding-plan", label),
    "account:zai-individual-coding-plan/GLM-5.3",
  );
  assert.equal(providerSwitchTarget(models, null, "api-key", label), "zai-standard-api/GLM-5.3-Flash");
  assert.equal(providerSwitchTarget(models, null, "other", label), undefined);
});

import assert from "node:assert/strict";
import test from "node:test";
import { nativeOptions, sessionAcpHarness, validAcpLaunch } from "../src/acp.ts";

test("native controls retain opaque IDs, grouped choices, and agent ordering", () => {
  const configuration = { configOptions: [
    { id: "model-id", name: "Choose model", category: "model", type: "select", currentValue: "a", options: [{ value: "a", name: "First" }, { group: "g", name: "Group", options: [{ value: "b", name: "Second" }] }] },
    { id: "thinking", name: "Effort", type: "select", currentValue: "native/high", options: [{ value: "native/high", name: "Think hard" }] },
    { id: "unknown", type: "boolean", currentValue: true },
  ] };
  const options = nativeOptions(configuration);
  assert.deepEqual(options.map((option) => option.id), ["model-id", "thinking"]);
  assert.deepEqual(options[0].choices.map((choice) => choice.id), ["a", "b"]);
  assert.equal(options[0].choices[1].description, "Group");
  const changed = nativeOptions({ configOptions: [{ ...configuration.configOptions[1], currentValue: "low", options: [{ value: "low", name: "Low" }] }] });
  assert.equal(changed[0].currentValue, "low");
  assert.equal(changed[0].choices.length, 1);
});

test("session labels and models survive removal of a saved harness", () => {
  const harness = sessionAcpHarness({ harness: "acp:stable-id", harnessName: "My agent", nativeConfiguration: { configOptions: [{ id: "m", name: "Model", category: "model", type: "select", currentValue: "native/model", options: [{ value: "native/model", name: "Native Model" }] }] } }, undefined);
  assert.equal(harness.name, "My agent");
  assert.equal(harness.agentReady, true);
  assert.deepEqual(harness.models.map((model) => model.displayName), ["Native Model"]);
  assert.equal(harness.options.permissionModes.length, 0);
});

test("modern options take precedence and unsupported controls keep defaults", () => {
  assert.deepEqual(nativeOptions({ configOptions: [], modes: { currentModeId: "plan", availableModes: [{ id: "plan", name: "Plan" }] } }), []);
  assert.deepEqual(nativeOptions(null), []);
  assert.equal(nativeOptions({ modes: { currentModeId: "ask", availableModes: [{ id: "ask", name: "Ask" }] } })[0].currentValue, "ask");
});

test("the settings form validates launch fields without interpreting argument boundaries", () => {
  assert.equal(validAcpLaunch("Agent", "dsh", ["--profile", "acp"]), true);
  assert.equal(validAcpLaunch("Agent", "/tmp/agent with spaces", ["two words", '\"quotes\"', "$(untouched)", ""]), true);
  for (const [name, executable, args] of [["", "agent", []], ["Agent", "  ", []], ["Agent", "./agent", []], ["Agent", "agent", ["bad\0argument"]]]) {
    assert.equal(validAcpLaunch(name, executable, args), false);
  }
});

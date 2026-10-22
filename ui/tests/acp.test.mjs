import assert from "node:assert/strict";
import test from "node:test";
import { formatAcpCommand, nativeOptions, parseAcpCommand, sessionAcpHarness, validAcpLaunch } from "../src/acp.ts";

test("a pasted ACP command preserves quoted paths and argument boundaries", () => {
  assert.deepEqual(parseAcpCommand('kimi acp'), { executable: 'kimi', arguments: ['acp'] });
  assert.deepEqual(parseAcpCommand('"/tmp/My Agent" --profile "two words" --empty \'\''), {
    executable: '/tmp/My Agent', arguments: ['--profile', 'two words', '--empty', ''],
  });
  assert.deepEqual(parseAcpCommand('C:\\Tools\\agent.exe --acp'), { executable: 'C:\\Tools\\agent.exe', arguments: ['--acp'] });
  assert.deepEqual(parseAcpCommand('agent --profile \\\n acp'), { executable: 'agent', arguments: ['--profile', 'acp'] });
});

test("saved launch configurations round trip without evaluating special characters", () => {
  const executable = "/tmp/agent's folder/agent";
  const args = ['two words', '"quoted"', "it's literal", '$HOME', '$(touch never)', '`touch never`', '', 'a; b', 'one\ntwo', 'C:\\Program Files\\agent'];
  assert.deepEqual(parseAcpCommand(formatAcpCommand(executable, args)), { executable, arguments: args });
});

test("incomplete commands and shell operations cannot silently become a different launch", () => {
  for (const command of ['', '   ', 'agent "unclosed', "agent 'unclosed", 'agent \\', 'agent | other', 'agent && other', 'agent; other', 'agent > output', 'agent < input', 'agent $(other)', 'agent $HOME', 'agent `other`', 'KEY=value agent', 'agent\nother', './agent --acp', 'agent "bad\0argument"']) {
    assert.equal(parseAcpCommand(command), null, command);
  }
});

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

test("an uninitialized ACP chat uses the discovered catalog until native options arrive", () => {
  const catalog = { id: "acp:stable-id", name: "Agent", models: [{ id: "a", displayName: "Model A" }, { id: "b", displayName: "Model B" }] };
  const session = { harness: catalog.id, nativeConfiguration: {} };
  assert.deepEqual(sessionAcpHarness(session, catalog).models, catalog.models);
  session.nativeConfiguration = { models: { currentModelId: "c", availableModels: [{ modelId: "c", name: "Session Model" }] } };
  assert.deepEqual(sessionAcpHarness(session, catalog).models.map((model) => model.id), ["c"]);
});

test("the settings form validates launch fields without interpreting argument boundaries", () => {
  assert.equal(validAcpLaunch("Agent", "dsh", ["--profile", "acp"]), true);
  assert.equal(validAcpLaunch("Agent", "/tmp/agent with spaces", ["two words", '\"quotes\"', "$(untouched)", ""]), true);
  for (const [name, executable, args] of [["", "agent", []], ["Agent", "  ", []], ["Agent", "./agent", []], ["Agent", "agent", ["bad\0argument"]]]) {
    assert.equal(validAcpLaunch(name, executable, args), false);
  }
});

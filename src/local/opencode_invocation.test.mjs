import test from "node:test";
import assert from "node:assert/strict";
import { Invocation } from "./opencode_invocation.mjs";

test("shell context follows the invoking model and provider without changing input", async () => {
  const messages = [
    { info: { role: "assistant", modelID: "model-a", providerID: "provider-a" }, parts: [{ type: "tool", callID: "call-a" }] },
    { info: { role: "assistant", modelID: "model-b", providerID: "provider-b" }, parts: [{ type: "tool", callID: "call-b" }] },
  ];
  const plugin = await Invocation({ client: { session: { messages: async () => ({ data: messages }) } } });
  for (const suffix of ["a", "b"]) {
    const input = { sessionID: "session", callID: `call-${suffix}`, command: "orx exp run experiment" };
    const original = structuredClone(input);
    const output = { env: {} };
    await plugin["shell.env"](input, output);
    assert.deepEqual(input, original);
    assert.deepEqual(JSON.parse(output.env.ORX_INVOCATION_CONTEXT), {
      harness: "opencode", model: `model-${suffix}`, provider: `provider-${suffix}`,
    });
  }
});

test("missing invoking model is reported", async () => {
  const plugin = await Invocation({ client: { session: { messages: async () => ({ data: [] }) } } });
  await assert.rejects(plugin["shell.env"]({ sessionID: "session", callID: "missing" }, { env: {} }));
});

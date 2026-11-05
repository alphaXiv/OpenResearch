export const Invocation = async ({ client }) => ({
  "shell.env": async (input, output) => {
    if (!input.sessionID || !input.callID) throw new Error("Missing native shell invocation identity");
    const result = await client.session.messages({ path: { id: input.sessionID } });
    const message = result.data?.find((message) =>
      message.info.role === "assistant" && message.parts.some((part) => part.type === "tool" && part.callID === input.callID),
    );
    if (!message?.info.modelID || !message.info.providerID) throw new Error("Missing native invoking model");
    output.env.ORX_INVOCATION_CONTEXT = JSON.stringify({
      harness: "opencode",
      model: message.info.modelID,
      provider: message.info.providerID,
    });
  },
});

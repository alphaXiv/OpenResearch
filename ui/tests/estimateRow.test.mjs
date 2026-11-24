import assert from "node:assert/strict";
import test from "node:test";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, renderChat, session, userMessage } from "./chat.mjs";

const bash = (id, command) => ({
  id, type: "tool", tool: "bash",
  state: { status: "completed", input: { command }, output: "" },
});

/** The chat's text once its agent ran `commands`, each in its own tool call. */
async function transcriptAfter(...commands) {
  await renderChat("/projects/p/tasks/s", {
    sessions: [session("s")],
    transcripts: {
      s: [
        userMessage("u1", "what would the child cost on an H100?"),
        assistantMessage("a1", commands.map((command, index) => bash(`tool-${index}`, command)), "u1"),
      ],
    },
  });
  return document.body.textContent;
}

test("an estimate reads as an estimate, never as a launch", async () => {
  const text = await transcriptAfter("orx exp run child --estimate --backend openresearch --flavor h100_sxm");
  assert.ok(text.includes(m.activity_estimated_run()), text);
  assert.ok(!text.includes(m.chat_panel_started_an_experiment_run()), text);
});

test("a launch chained after an estimate still reads as a launch", async () => {
  const text = await transcriptAfter("orx exp run child --estimate --backend hf --flavor h200 && orx exp run child --backend hf --flavor h200");
  assert.ok(text.includes(m.chat_panel_started_an_experiment_run()), text);
});

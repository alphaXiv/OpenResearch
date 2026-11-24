import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { renderChat, session } from "./chat.mjs";
import { settle } from "./render.mjs";

test("each click on a workspace tool reports the pane it opens", async () => {
  const chat = await renderChat("/projects/p/tasks/s", { sessions: [session("s")] });
  const tool = (label) => document.querySelector(`.workspace-tools button[aria-label="${label}"]`);
  for (const label of [m.app_files(), m.app_artifacts(), m.app_files(), m.app_experiments()]) {
    await act(async () => tool(label).click());
    await settle();
  }
  assert.deepEqual(
    chat.requests.filter(({ request }) => request === "POST /api/telemetry/event").map(({ body }) => body).filter(({ name }) => name === "pane_opened"),
    ["files", "artifacts", "files", "experiments"].map((pane) => ({ name: "pane_opened", pane })),
  );
});

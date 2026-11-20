import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, renderChat, session, userMessage } from "./chat.mjs";
import { model } from "./models.mjs";
import { settle } from "./render.mjs";

const claude = {
  id: "claude-code", name: "Claude Code", installed: true, installBroken: false, authenticated: true, authState: "ready", agentReady: true,
  supportsSteering: true, supportsCompaction: true, models: [model("opus")], defaultModel: "opus", fullAccessNeedsRootConsent: true,
  options: {
    permissionModes: [{ id: "auto", label: "Auto" }, { id: "bypassPermissions", label: "Bypass permissions" }],
    defaultPermissionMode: "auto", planActivation: "permission", reasoningLevels: [],
  },
};

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};
const button = (text) => [...document.querySelectorAll("button")].find((element) => element.textContent === text);
const dialog = () => document.querySelector('[role="dialog"]');
const sent = (chat, request) => chat.requests.filter((entry) => entry.request === request).map(({ body }) => body ?? {});

/** Chat `s` on Claude Code as root, its last reply an end-turn approval card. */
async function rootChat() {
  const card = { id: "approve", type: "prompt", prompt: { kind: "permission", tool: "Bash", toolInput: { command: "rm -rf build" } } };
  return renderChat("/projects/p/tasks/s", {
    sessions: [session("s", { harness: "claude-code", model: "opus" })],
    transcripts: { s: [userMessage("u1", "clean the build"), assistantMessage("a1", [card], "u1")] },
    harnesses: [claude],
    routes: {
      "POST /api/harnesses/claude-code/root-full-access": () => ({}),
      "POST /api/chat/sessions/s/respond": () => ({ ok: true }),
    },
  });
}

test("as root, an approval that bypasses permissions asks first and is remembered", async () => {
  const chat = await rootChat();

  await act(async () => button(m.chat_panel_allow()).focus());
  await click(button(m.chat_panel_allow()));
  assert.equal(dialog()?.textContent.includes(m.chat_root_bypass_title()), true);
  await click(button(m.common_cancel()));
  await act(async () => new Promise((resolve) => setTimeout(resolve, 300)));
  assert.equal(document.activeElement, button(m.chat_panel_allow()));
  assert.deepEqual(sent(chat, "POST /api/chat/sessions/s/respond"), []);
  assert.deepEqual(sent(chat, "POST /api/harnesses/claude-code/root-full-access"), []);

  await click(button(m.chat_panel_allow()));
  await click(button(m.chat_root_bypass_confirm()));
  assert.equal(sent(chat, "POST /api/harnesses/claude-code/root-full-access").length, 1);
  assert.deepEqual(sent(chat, "POST /api/chat/sessions/s/respond"), [{ promptId: "approve", approve: true }]);
});

test("as root, choosing Bypass permissions asks first and saves the mode once confirmed", async () => {
  const chat = await rootChat();
  const pill = [...document.querySelectorAll("button")].find((element) => element.getAttribute("aria-label")?.startsWith(m.model_picker_permissions()));
  await click(pill);
  const bypass = [...document.querySelectorAll('[role="menuitemradio"]')].find((element) => element.textContent.includes("Bypass permissions"));
  await click(bypass);
  assert.deepEqual(sent(chat, "PATCH /api/chat/sessions/s"), []);
  await click(button(m.chat_root_bypass_confirm()));
  assert.equal(sent(chat, "POST /api/harnesses/claude-code/root-full-access").length, 1);
  assert.deepEqual(sent(chat, "PATCH /api/chat/sessions/s"), [{ permissionMode: "bypassPermissions" }]);
});

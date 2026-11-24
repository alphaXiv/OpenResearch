import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { SkillsTab } from "../src/components/SkillsTab.tsx";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, renderChat, session, userMessage } from "./chat.mjs";
import { find, render, serve, settle, type } from "./render.mjs";

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};
const file = (content, version) => ({ content, version, path: "/home/me/.config/openresearch/AGENTS.md" });

test("a save that loses to an outside edit keeps the draft and shows the newer file", async () => {
  let onDisk = file("Name runs after the idea.", "v1");
  const saves = [];
  serve((url, init) => {
    if (url === "/api/user-skills") return { skills: [] };
    if (url === "/api/latex-templates") return { templates: [] };
    if (url !== "/api/settings/instructions") return;
    if (init.method !== "PUT") return onDisk;
    const body = JSON.parse(init.body);
    saves.push(body);
    if (body.baseVersion !== onDisk.version) {
      return Response.json({ error: "changed", code: "fileChanged", currentVersion: onDisk.version, exists: true }, { status: 409 });
    }
    onDisk = file(body.content, `v${saves.length + 1}`);
    return onDisk;
  });
  const { container } = await render(createElement(SkillsTab));
  await settle();
  const editor = container.querySelector("textarea");
  const save = () => find("button", m.common_save());
  assert.equal(editor.value, "Name runs after the idea.");
  assert.equal(save().disabled, true);

  await type(editor, "Mine.");
  onDisk = file("Theirs.", "v9");
  await click(save());
  assert.deepEqual(saves, [{ content: "Mine.", baseVersion: "v1" }]);
  assert.equal(container.querySelector("textarea").value, "Mine.");
  const notice = find('[role="alert"]', m.instructions_changed());
  assert.ok(notice.textContent.includes("Theirs."));
  assert.equal(save().disabled, true);

  await click(find("button", m.instructions_keep_mine()));
  assert.equal(find('[role="alert"]', m.instructions_changed()), null);
  await click(save());
  assert.deepEqual(saves.at(-1), { content: "Mine.", baseVersion: "v9" });
  assert.equal(onDisk.content, "Mine.");

  await type(container.querySelector("textarea"), "");
  await click(save());
  assert.equal(onDisk.content, "", "an empty editor saves, clearing the instructions");
});

const proposal = (instructionsVersion) => ({
  id: "instructions_1",
  type: "prompt",
  prompt: {
    kind: "instructions",
    instructionsVersion,
    diff: "diff --git a/AGENTS.md b/AGENTS.md\n--- a/AGENTS.md\n+++ b/AGENTS.md\n@@ -0,0 +1,1 @@\n+Report results in a table.\n",
  },
});

async function proposed(onDiskVersion) {
  const chat = await renderChat("/projects/p/tasks/s", {
    transcripts: { s: [userMessage("u1", "remember to use tables"), assistantMessage("a1", [proposal("v1")], "u1")] },
    routes: {
      "GET /api/settings/instructions": () => file("", onDiskVersion),
      "POST /api/chat/sessions/s/respond": () => ({ ok: true }),
    },
  });
  await settle();
  const card = () => document.querySelector("section.instructions");
  return {
    card,
    button: (label) => [...card().querySelectorAll("button")].find((element) => element.textContent === label),
    answers: () => chat.requests.filter(({ request }) => request === "POST /api/chat/sessions/s/respond").map(({ body }) => body),
  };
}

test("an instructions card shows the change and approves it", async () => {
  const view = await proposed("v1");
  assert.ok(view.card().textContent.includes("Report results in a table."));
  await click(view.button(m.chat_instructions_approve()));
  assert.deepEqual(view.answers(), [{ promptId: "instructions_1", approve: true }]);
});

test("an instructions card made against an older file offers only Decline", async () => {
  const view = await proposed("v2");
  assert.ok(view.card().textContent.includes(m.chat_instructions_stale()));
  assert.equal(view.button(m.chat_instructions_approve()), undefined);
  await click(view.button(m.chat_instructions_decline()));
  assert.deepEqual(view.answers(), [{ promptId: "instructions_1", approve: false }]);
});

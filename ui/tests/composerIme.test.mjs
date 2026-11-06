import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { Message } from "../src/components/ChatPanel.tsx";
import { PlanStrip } from "../src/components/PlanStrip.tsx";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, codex, composer, renderChat, session, settle, userMessage } from "./chat.mjs";
import { find, render, type } from "./render.mjs";

const composing = [{ isComposing: true }, { keyCode: 229 }];
const skills = [
  { name: "zza", description: "Unavailable", disabled: true, source: "user" },
  { name: "zzb", description: "Second", source: "user" },
  { name: "zzc", description: "Unavailable", disabled: true, source: "user" },
  { name: "zzd", description: "Fourth", source: "user" },
];

/** Dispatches a keydown on `target`; reports whether it was handled and whether it reached the window. */
async function key(target, name, init = {}) {
  let bubbled = false;
  const listen = () => { bubbled = true; };
  window.addEventListener("keydown", listen);
  const event = new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true, ...init });
  await act(async () => {
    target.focus();
    target.dispatchEvent(event);
  });
  window.removeEventListener("keydown", listen);
  await settle();
  return { prevented: event.defaultPrevented, bubbled };
}

/** The chat composer, over a backend that records what it sends. */
async function chatEditor({ busy = false, harness = codex, sessions = [session("s", { busy })], routes = {} } = {}) {
  const chat = await renderChat("/projects/p/tasks/s", {
    sessions, harnesses: [harness], transcripts: { s: [userMessage("u1", "hello"), assistantMessage("a1", "Hi.", "u1")] },
    routes: {
      "GET /api/skills": () => ({ skills }),
      "GET /api/projects/p/code-tree": () => ({ root: "repo", branch: "main", entries: ["notes.md"] }),
      "POST /api/chat/sessions/s/message": () => ({ ok: true }),
      ...routes,
    },
  });
  const input = composer();
  return {
    chat, input, field: input.field,
    submitted: () => chat.sent().map(({ text }) => text),
    requests: (pattern) => chat.requests.filter(({ request }) => pattern.test(request)),
  };
}

/** One of the three text editors that submit on Enter, holding "draft". */
const editors = {
  async composer() {
    const editor = await chatEditor();
    await editor.input.type("draft");
    return { field: editor.field, submitted: editor.submitted };
  },
  async "edit/resend"() {
    const submitted = [];
    await render(createElement(Message, {
      message: userMessage("u1", "draft"), forkCount: 1, forkIndex: 0, forkDisabled: false, branchDisabled: false,
      onFork: (_, text) => submitted.push(text), onSelectFork: () => {}, onRecover: () => {},
    }));
    await act(async () => document.querySelector(`button[aria-label="${m.chat_panel_edit_and_re_send()}"]`).click());
    const field = () => document.querySelector(`textarea[aria-label="${m.chat_panel_edit_message()}"]`);
    return { field, submitted: () => submitted };
  },
  async "plan revision"() {
    const submitted = [];
    await render(createElement(PlanStrip, {
      synthesized: false, agentLabel: "Codex", showResumeModes: false,
      onView: () => {}, onApprove: () => {}, onReject: () => {}, onRevise: (note) => submitted.push(note),
    }));
    await act(async () => find("button", m.plan_strip_revise_05bacc9()).click());
    const field = () => document.querySelector(`textarea[placeholder="${m.plan_strip_what_should_change_optional()}"]`);
    await type(field(), "draft");
    return { field, submitted: () => submitted };
  },
};

for (const [name, open] of Object.entries(editors)) {
  for (const init of composing) {
    test(`${name} leaves IME Enter to the input method: ${JSON.stringify(init)}`, async () => {
      const editor = await open();
      const ime = await key(editor.field(), "Enter", init);
      assert.deepEqual(editor.submitted(), []);
      assert.deepEqual(ime, { prevented: false, bubbled: false });
      const enter = await key(editor.field(), "Enter");
      assert.equal(enter.prevented, true);
      assert.deepEqual(editor.submitted(), ["draft"]);
    });
  }

  test(`${name} preserves Shift+Enter`, async () => {
    const editor = await open();
    await key(editor.field(), "Enter", { shiftKey: true });
    assert.deepEqual(editor.submitted(), []);
  });

  test(`IME Escape in ${name} cannot bubble to the stop-turn shortcut`, async () => {
    const editor = await open();
    for (const init of composing) {
      assert.deepEqual(await key(editor.field(), "Escape", init), { prevented: false, bubbled: false });
    }
    assert.ok(editor.field());
  });
}

test("IME Escape in a running chat's composer does not stop the turn", async () => {
  const editor = await chatEditor({ busy: true, routes: { "POST /api/chat/sessions/s/interrupt": () => ({ ok: true }) } });
  for (const init of composing) await key(editor.field(), "Escape", init);
  assert.deepEqual(editor.requests(/interrupt$/), []);
  await key(editor.field(), "Escape");
  assert.equal(editor.requests(/interrupt$/).length, 1);
});

test("composition ending before WebKit's confirming keydown does not submit", async () => {
  const editor = await chatEditor();
  await editor.input.type("draft");
  await act(async () => editor.field().dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true })));
  await key(editor.field(), "Enter", { isComposing: true });
  assert.deepEqual(editor.submitted(), []);
  await act(async () => editor.field().dispatchEvent(new CompositionEvent("compositionend", { bubbles: true })));
  await key(editor.field(), "Enter", { keyCode: 229 });
  assert.deepEqual(editor.submitted(), []);
  await key(editor.field(), "Enter");
  assert.deepEqual(editor.submitted(), ["draft"]);
});

for (const [menu, draft, picked] of [["skill", "/zz", "/zzb"], ["mention", "@no", "@notes.md"]]) {
  test(`IME keys do not activate ${menu} menu shortcuts`, async () => {
    const editor = await chatEditor();
    await editor.input.type(draft);
    for (const name of ["Tab", "ArrowDown", "Escape", "Enter"]) {
      for (const init of composing) assert.equal((await key(editor.field(), name, init)).bubbled, false, name);
    }
    assert.equal(editor.input.draft, draft);
    await key(editor.field(), "Enter");
    assert.ok(editor.input.draft.startsWith(picked), editor.input.draft);
    assert.deepEqual(editor.submitted(), []);
  });
}

for (const platform of ["MacIntel", "Win32"]) {
  test(`${platform === "MacIntel" ? "Cmd" : "Ctrl"}+Enter queues behind a running turn instead of steering`, async (t) => {
    Object.defineProperty(navigator, "platform", { configurable: true, value: platform });
    t.after(() => delete navigator.platform);
    const editor = await chatEditor({ busy: true, harness: { ...codex, supportsSteering: true } });
    await editor.input.type("park this");
    await key(editor.field(), "Enter", platform === "MacIntel" ? { metaKey: true } : { ctrlKey: true });
    await editor.input.type("steer now");
    await key(editor.field(), "Enter");
    assert.deepEqual(editor.chat.sent().map(({ text, mode }) => [text, mode]), [["park this", undefined], ["steer now", "steer"]]);
  });
}

test("bash mode runs its command on Enter", async () => {
  const editor = await chatEditor({ routes: { "POST /api/chat/sessions/s/shell": () => ({ message: userMessage("shell", "!ls") }) } });
  await editor.input.type("!ls");
  await key(editor.field(), "Enter");
  assert.deepEqual(editor.requests(/\/shell$/).map(({ body }) => body), [{ command: "ls" }]);
  assert.deepEqual(editor.submitted(), []);
});

for (const [menu, draft, picked] of [["skill", "/zz", "/zzb"], ["mention", "@no", "@notes.md"]]) {
  test(`an open ${menu} menu picks its item on Enter`, async () => {
    const editor = await chatEditor();
    await editor.input.type(draft);
    await key(editor.field(), "Enter");
    assert.ok(editor.input.draft.startsWith(picked), editor.input.draft);
    assert.deepEqual(editor.submitted(), []);
  });
}

test("slash menu selection and arrow keys skip disabled commands", async () => {
  for (const [keys, picked] of [[[], "/zzb"], [["ArrowDown"], "/zzd"], [["ArrowDown", "ArrowDown"], "/zzb"], [["ArrowUp"], "/zzd"]]) {
    const editor = await chatEditor();
    await editor.input.type("/zz");
    for (const name of keys) assert.equal((await key(editor.field(), name)).prevented, true);
    await key(editor.field(), "Enter");
    assert.ok(editor.input.draft.startsWith(picked), `${keys}: ${editor.input.draft}`);
    await editor.chat.unmount();
  }
});

test("a menu containing only disabled commands leaves Tab available for focus navigation", async () => {
  const editor = await chatEditor();
  await editor.input.type("/zza");
  assert.ok(find("button", "Unavailable").disabled);
  assert.equal((await key(editor.field(), "Tab")).prevented, false);
  assert.equal((await key(editor.field(), "ArrowDown")).prevented, true);
  assert.equal(editor.input.draft, "/zza");
});

for (const command of ["/compact", "/summarize"]) {
  test(`Enter on typed unsupported ${command} silently does nothing`, async () => {
    const editor = await chatEditor({ harness: { ...codex, supportsCompaction: false } });
    await editor.input.type(command);
    await key(editor.field(), "Enter");
    assert.deepEqual(editor.requests(/^POST \/api\/chat/), []);
    assert.equal(document.querySelector('[role="alert"]')?.outerHTML, undefined);
  });
}

test("activating Goal removes its token without moving the caret behind newly typed text", async () => {
  const editor = await chatEditor();
  let frames = [];
  const { requestAnimationFrame } = window;
  window.requestAnimationFrame = (callback) => frames.push(callback);
  try {
    await editor.input.type("/goal ");
    assert.equal(editor.input.draft, "");
    await editor.input.type("R");
    await act(async () => { for (const frame of frames.splice(0)) frame(0); });
  } finally {
    window.requestAnimationFrame = requestAnimationFrame;
  }
  assert.equal(editor.input.draft, "R");
  assert.equal(editor.input.caret, 1);
});

for (const succeeds of [false, true]) {
  test(`ACP Plan configuration ${succeeds ? "success sends the message" : "failure keeps the draft and prevents sending"}`, async () => {
    const acp = session("s", { harness: "acp:agent", harnessName: "Agent", nativeConfiguration: { configOptions: [{
      type: "select", id: "mode", name: "Mode", category: "mode", currentValue: "code",
      options: [{ value: "code", name: "Code" }, { value: "plan", name: "Plan" }],
    }] } });
    const editor = await chatEditor({ sessions: [acp], routes: {
      "POST /api/chat/sessions/s/configuration": () => succeeds ? { ...acp, nativeConfiguration: acp.nativeConfiguration } : Response.json({ error: "disconnected" }, { status: 502 }),
    } });
    await editor.input.type("/plan sketch it");
    await key(editor.field(), "Enter");
    assert.equal(editor.requests(/\/configuration$/).length, 1);
    assert.deepEqual(editor.submitted(), succeeds ? ["sketch it"] : []);
    if (!succeeds) {
      assert.ok(document.body.textContent.includes(m.chat_toggle_plan_failed()));
      assert.equal(editor.input.draft, "/plan sketch it");
    }
  });
}

for (const [supportsCompaction, path] of [[false, "/projects/p/tasks/new"], [false, "/projects/p/tasks/s"], [true, "/projects/p/tasks/s"]]) {
  test(`compacting ${path.endsWith("new") ? "a new" : "a saved"} chat ${supportsCompaction ? "asks the agent" : "on a harness without compaction does nothing"}`, async () => {
    const chat = await renderChat(path, {
      harnesses: [{ ...codex, supportsCompaction }],
      transcripts: { s: [userMessage("u1", "hello"), assistantMessage("a1", "Hi.", "u1")] },
      routes: { "POST /api/chat/sessions/s/compact": () => ({ message: assistantMessage("marker", "Context compacted.", "a1") }) },
    });
    await composer().type("/compact ");
    const compactions = chat.requests.filter(({ request }) => request.endsWith("/compact"));
    assert.equal(compactions.length, supportsCompaction ? 1 : 0);
    assert.doesNotMatch(document.body.textContent, new RegExp(m.chat_nothing_to_compact()));
  });
}

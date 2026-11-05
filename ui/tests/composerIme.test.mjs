import { matchesKeyboardEvent } from "@tanstack/react-hotkeys";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const file = ts.createSourceFile("ChatPanel.tsx", readFileSync(new URL("../src/components/ChatPanel.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);

function textareaCallback(editor, name) {
  const source = editor === "plan"
    ? ts.createSourceFile("PlanStrip.tsx", readFileSync(new URL("../src/components/PlanStrip.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
    : file;
  let callback;
  function visit(node) {
    if (ts.isJsxSelfClosingElement(node) && node.tagName.getText(source) === (editor ? "textarea" : "ComposerEditor")) {
      const attributes = node.attributes.properties;
      const target = !editor || attributes.some(attribute => ts.isJsxAttribute(attribute)
        && attribute.name.text === "value"
        && attribute.initializer?.expression?.getText(source) === (editor === "plan" ? "note" : "editDraft"));
      if (target) callback = attributes.find(attribute => attribute.name?.text === name)?.initializer.expression;
    }
    ts.forEachChild(node, visit);
  }
  visit(source);
  assert.ok(callback, name);
  return callback.getText(source);
}

function composer(editor, overrides = {}, platform = "mac") {
  const calls = [];
  let stopped = false;
  const record = name => (...args) => calls.push([name, ...args]);
  const bindings = {
    matchesKeyboardEvent: (event, hotkey) => matchesKeyboardEvent(event, hotkey, platform),
    composingRef: { current: false }, skillMenuOpen: false, mentionMenuOpen: false, bashMode: false,
    activeSkillIdx: 0, selectableSkillIndices: [0], skillMatches: ["skill"], activeMentionIdx: 0, mentionMatches: ["file"], mentionQuery: "file",
    setSkillIdx: record("skillIndex"), pickSkill: record("skill"), setMentionIdx: record("mentionIndex"),
    pickMention: record("mention"), setSkillMenuDismissed: record("dismiss"),
    deleteCommandBehindCaret: record("deleteChip"), runShell: record("shell"), send: record("send"),
    submit: record("submit"), submitRevision: record("submit"), setEditDraft: record("edit"),
    setNote: record("note"), setRevising: record("revising"), ...overrides,
  };
  const evaluate = name => new Function(...Object.keys(bindings), `return (${textareaCallback(editor, name)})`)(...Object.values(bindings));
  const keydown = evaluate("onKeyDown");
  // The composer editor drops composing keys before its handlers, then sends a
  // plain Enter its key handler left alone.
  const submit = editor ? null : evaluate("onSubmit");
  return {
    calls, evaluate,
    get stopped() { return stopped; },
    press(key = "Enter", nativeEvent = {}, modifiers = {}) {
      stopped = false;
      if (!editor) {
        const event = { key, shiftKey: false, metaKey: false, ctrlKey: false, altKey: false, isComposing: false, keyCode: 13, ...nativeEvent, ...modifiers,
          defaultPrevented: false,
          stopPropagation: () => { stopped = true; },
          preventDefault: () => { event.defaultPrevented = true; calls.push(["preventDefault"]); } };
        keydown(event);
        if (!event.defaultPrevented && key === "Enter" && !event.shiftKey) {
          event.preventDefault();
          submit(event);
        }
        return;
      }
      keydown({ key, shiftKey: false, metaKey: false, ctrlKey: false, ...modifiers,
        stopPropagation: () => { stopped = true; },
        nativeEvent: { key, shiftKey: false, metaKey: false, ctrlKey: false, altKey: false, ...modifiers, isComposing: false, keyCode: 13, ...nativeEvent }, preventDefault: record("preventDefault") });
    },
  };
}

for (const editor of [true, "plan"]) {
  for (const nativeEvent of [{ isComposing: true }, { keyCode: 229 }]) {
    test(`${editor === "plan" ? "plan revision" : editor ? "edit/resend" : "composer"} leaves IME Enter to the input method: ${JSON.stringify(nativeEvent)}`, () => {
      const input = composer(editor);
      input.press("Enter", nativeEvent);
      assert.deepEqual(input.calls, []);
      assert.equal(input.stopped, true);
      input.press();
      assert.deepEqual(input.calls, [["preventDefault"], editor ? ["submit"] : ["send", { queue: false }]]);
    });
  }
}

for (const editor of [false, true, "plan"]) {
  test(`${editor === "plan" ? "plan revision" : editor ? "edit/resend" : "composer"} sends on Enter and preserves Shift+Enter`, () => {
    const input = composer(editor);
    input.press("Enter", {}, { shiftKey: true });
    assert.deepEqual(input.calls, []);
    input.press();
    assert.deepEqual(input.calls, [["preventDefault"], editor ? ["submit"] : ["send", { queue: false }]]);
  });
}

for (const editor of [true, "plan"]) {
  test(`IME Escape cannot bubble to the stop-turn shortcut (${editor})`, () => {
    const input = composer(editor, { skillMenuOpen: true });
    for (const nativeEvent of [{ isComposing: true }, { keyCode: 229 }]) {
      input.press("Escape", nativeEvent);
      assert.equal(input.stopped, true);
      assert.deepEqual(input.calls, []);
    }
  });
}

for (const modifier of ["metaKey", "ctrlKey"]) {
  test(`${modifier}+Enter still queues outside composition`, () => {
    const input = composer(false, {}, modifier === "metaKey" ? "mac" : "windows");
    input.press("Enter", {}, { [modifier]: true });
    assert.deepEqual(input.calls, [["preventDefault"], ["send", { queue: true }]]);
  });
}

for (const [mode, action] of [["bashMode", "shell"], ["skillMenuOpen", "skill"], ["mentionMenuOpen", "mention"]]) {
  test(`${mode} still handles normal Enter`, () => {
    const input = composer(false, { [mode]: true });
    input.press();
    assert.deepEqual(input.calls, [["preventDefault"], action === "shell" ? [action] : [action, action === "skill" ? "skill" : "file"]]);
  });
}

function skillSelection(skillMatches, skillIdx = 0) {
  const names = ["selectableSkillIndices", "activeSkillIdx"];
  const declarations = [];
  function find(node) {
    if (ts.isVariableDeclaration(node) && names.includes(node.name.getText(file))) declarations.push(`const ${node.getText(file)};`);
    ts.forEachChild(node, find);
  }
  find(file);
  return new Function("skillMatches", "skillIdx", `${declarations.join("\n")} return { selectableSkillIndices, activeSkillIdx };`)(skillMatches, skillIdx);
}

test("slash menu selection and arrow keys skip disabled commands", () => {
  const skillMatches = [{ disabled: true }, { name: "copy" }, { disabled: true }, { name: "export" }];
  assert.deepEqual(skillSelection(skillMatches), { selectableSkillIndices: [1, 3], activeSkillIdx: 1 });
  assert.equal(skillSelection(skillMatches, 2).activeSkillIdx, 1);
  for (const [index, key, expected] of [[1, "ArrowDown", 3], [3, "ArrowDown", 1], [1, "ArrowUp", 3], [3, "ArrowUp", 1]]) {
    const input = composer(false, { skillMenuOpen: true, skillMatches, ...skillSelection(skillMatches, index) });
    input.press(key);
    assert.deepEqual(input.calls, [["preventDefault"], ["skillIndex", expected]]);
  }
});

test("a menu containing only disabled commands leaves Tab available for focus navigation", () => {
  const skillMatches = [{ name: "compact", disabled: true }];
  const selection = skillSelection(skillMatches);
  assert.deepEqual(selection, { selectableSkillIndices: [], activeSkillIdx: -1 });
  const input = composer(false, { skillMenuOpen: true, skillMatches, ...selection });
  input.press("Tab");
  assert.deepEqual(input.calls, []);
  input.press("ArrowDown");
  assert.deepEqual(input.calls, [["preventDefault"]]);
});

test("Enter on typed unsupported compact and its alias silently does nothing", async () => {
  const { parseComposerCommand, takesArgument } = await import("../src/composerCommands.ts");
  const declarations = [];
  function find(node) {
    if (ts.isFunctionDeclaration(node) && ["send", "runComposerCommand", "compactSession"].includes(node.name?.text)) declarations.push(node.getText(file));
    ts.forEachChild(node, find);
  }
  find(file);
  const compiled = ts.transpileModule(declarations.join("\n"), { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  for (const draft of ["/compact", "/summarize"]) {
    const calls = [];
    const bindings = {
    matchesKeyboardEvent: (event, hotkey) => matchesKeyboardEvent(event, hotkey, platform),
      draft, planActivation: null, goalArmed: false, parseComposerCommand, takesArgument,
      setChatSessionAgentMutation: { isPending: false },
      setDraft: () => {}, setSkillMenuDismissed: () => {},
      activeId: "saved-chat", activeHarness: { supportsCompaction: false },
      m: { chat_compact_unsupported: () => "unsupported" },
      setSettingsError: (error) => calls.push(error),
      compactChatSession: () => calls.push("compact request"), sendChatMessage: () => calls.push("message request"),
    };
    const send = new Function(...Object.keys(bindings), `${compiled}; return send;`)(...Object.values(bindings));
    const skillMatches = [{ name: "compact", disabled: true }];
    const input = composer(false, { skillMenuOpen: true, skillMatches, ...skillSelection(skillMatches), send });
    input.press();
    assert.deepEqual(input.calls, [["preventDefault"]]);
    assert.deepEqual(calls, []);
  }
});

test("activating Goal removes its token in one editor edit", async () => {
  const { removeSlashCommand } = await import("../src/composerCommands.ts");
  let activation;
  function find(node) {
    if (ts.isFunctionDeclaration(node) && node.name?.text === "activateComposerCommand") activation = node;
    ts.forEachChild(node, find);
  }
  find(file);
  assert.ok(activation);
  const compiled = ts.transpileModule(`const activate = ${activation.getText(file)};`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
  }).outputText;
  const calls = [];
  const bindings = {
    removeSlashCommand,
    composerRef: { current: { setText: (...edit) => calls.push(["setText", ...edit]) } },
    setDraft: (text) => calls.push(["draft", text]),
    setSkillMenuDismissed: () => {},
    runComposerCommand: (name) => { calls.push(name); return false; },
  };
  const activate = new Function(...Object.keys(bindings), `${compiled}\nreturn activate;`)(...Object.values(bindings));
  activate("goal", "/goal now", { start: 0, end: 5, query: "goal" });
  assert.deepEqual(calls, [["draft", "now"], "goal", ["setText", "now", 0]]);
});

test("ACP Plan configuration failure keeps the draft and prevents sending", async () => {
  let configure;
  function find(node) {
    if (ts.isIfStatement(node) && node.expression.getText(file) === "planRequested && acpMode && !retry") configure = node;
    ts.forEachChild(node, find);
  }
  find(file);
  assert.ok(configure);
  for (const succeeds of [false, true]) {
    const calls = [];
    const bindings = {
    matchesKeyboardEvent: (event, hotkey) => matchesKeyboardEvent(event, hotkey, platform),
      planRequested: true, acpMode: {}, retry: null, toggledPlanMode: true,
      setIndependentPlanMode: async () => { if (!succeeds) throw new Error("disconnected"); },
      inSourceScope: () => true,
      setSettingsError: error => calls.push(error),
      m: { chat_toggle_plan_failed: () => "plan failed" },
      calls,
    };
    const compiled = ts.transpileModule(`async function send() { ${configure.getText(file)}; calls.push("send"); }`, {
      compilerOptions: { target: ts.ScriptTarget.ES2022 },
    }).outputText;
    await new Function(...Object.keys(bindings), `${compiled}; return send();`)(...Object.values(bindings));
    assert.deepEqual(calls, succeeds ? ["send"] : ["plan failed"]);
  }
});

test("unsupported compaction silently skips the agent turn", async () => {
  let compact;
  function find(node) {
    if (ts.isFunctionDeclaration(node) && node.name?.text === "compactSession") compact = node;
    ts.forEachChild(node, find);
  }
  find(file);
  assert.ok(compact);
  for (const [supportsCompaction, activeId] of [[false, null], [false, "saved-chat"], [true, "saved-chat"]]) {
    const calls = [];
    const bindings = {
    matchesKeyboardEvent: (event, hotkey) => matchesKeyboardEvent(event, hotkey, platform),
      activeId, activeHarness: { supportsCompaction }, busy: false,
      m: { chat_nothing_to_compact: () => "nothing to compact" },
      showAlert: (message) => calls.push(message),
      setSettingsError: (error) => calls.push(error),
      sessionFilter: "active", composerScopeRef: { current: { activeId: "saved-chat" } },
      compactChatSession: async (id) => { calls.push(["compact", id]); return { message: { id: "marker" } }; },
      dispatch: (action) => calls.push(action),
    };
    const compiled = ts.transpileModule(compact.getText(file), {
      compilerOptions: { target: ts.ScriptTarget.ES2022 },
    }).outputText;
    await new Function(...Object.keys(bindings), `${compiled}; return compactSession();`)(...Object.values(bindings));
    assert.deepEqual(calls, supportsCompaction ? [
      null, ["compact", "saved-chat"],
      { type: "upsertMessage", sessionId: "saved-chat", message: { id: "marker" }, onlyIfMissing: true },
    ] : []);
  }
});

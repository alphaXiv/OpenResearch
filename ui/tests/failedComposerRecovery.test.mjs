import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import { composerStashContent, EMPTY_COMPOSER_STASH } from "../src/composerStash.ts";
import { queryModules } from "./queryModules.mjs";

const file = ts.createSourceFile("ChatPanel.tsx", readFileSync(new URL("../src/components/ChatPanel.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
function find(name) {
  let found;
  function visit(node) {
    if (ts.isFunctionDeclaration(node) && node.name?.text === name) found = node.getText(file);
    ts.forEachChild(node, visit);
  }
  visit(file);
  assert.ok(found, name);
  return found;
}
const code = ts.transpileModule(`${find("mergeFailedComposer")}\n${find("send")}\nreturn send;`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;

class RejectedResponseError extends Error {}

function setup({ side = false, sessionId = "s", busy = false, steering = false, history = async () => ({}), submission = async () => ({ accepted: false }), deferred = false } = {}) {
  let rejectSend, resolveSend;
  let state = { draft: "original", attachments: [{ dataUrl: "data:,old", mediaType: "image/png", size: 1 }], annotations: [{ id: "a", text: "old selection" }], unconfirmedSend: null };
  const composerLiveRef = { current: state };
  const updates = [];
  const stashes = new Map(), mounted = new Map(), actions = [], deleted = new Set();
  let generation = 1;
  const key = sessionId ? `1:${side ? "side:" : ""}${sessionId}` : `1:p:${side ? "side:parent" : "new"}`;
  const scope = { current: { projectId: "p", activeId: sessionId, mainView: "chat" } };
  const update = (field) => (value) => {
    const apply = () => { state = { ...state, [field]: typeof value === "function" ? value(state[field]) : value }; };
    if (deferred) updates.push(apply);
    else apply();
  };
  const bindings = {
    draft: state.draft, attachments: state.attachments, annotations: state.annotations,
    stashKey: key, stashScope: "1:", embedded: side, sideParentSessionId: side ? "parent" : null,
    projectId: "p", mainView: "chat", activeId: sessionId, composerScopeRef: scope, composerLiveRef,
    composerMountedRef: { current: true }, sessionsOptions: { queryKey: ["workspace", 1] },
    isCurrentScope: (key) => key[1] === generation, deletedSessionIds: deleted,
    composerStashes: stashes, mountedComposers: mounted, composerStashContent, EMPTY_COMPOSER_STASH,
    setDraft: update("draft"), setAttachments: update("attachments"), setAnnotations: update("annotations"),
    setUnconfirmedSend: update("unconfirmedSend"),
    setAttachError: () => {}, setImageRefusedFor: () => {}, setGoalScope: () => {}, setSkillMenuDismissed: () => {},
    parseComposerCommand: () => null, planActivation: null, goalArmed: false, takesArgument: () => false,
    imageInputBlocked: false, captureUiEvent: () => {}, telemetrySurface: "chat", preparingSend: { current: false },
    planActive: false, effectiveCommandPlanMode: () => undefined, acpMode: false, activeHarness: { id: "codex", agentReady: true },
    composerSelection: { harness: "codex" }, planModeOverrideRef: { current: null }, planMutationSeq: { current: 0 },
    RejectedResponseError, pendingClientTurn: { current: null }, pendingScope: { current: null }, crypto, busy, steering,
    queueSessionMutation: (fn) => fn(), setSessionOverride: () => {}, setRecoveryOverrides: () => {}, EMPTY_RECOVERY_OVERRIDES: {},
    sendChatMessage: () => new Promise((resolve, reject) => { resolveSend = resolve; rejectSend = reject; }), getChatSubmission: submission,
    openNewSession: async () => ({ id: "created" }), getChatMessagesQuery: () => ({ queryKey: [] }),
    queryClient: { fetchQuery: history, getQueryState: () => undefined }, listChatSessionsQuery: () => ({}),
    dispatch: (action) => actions.push(action), pinTranscriptToBottom: () => {}, sessionFilter: "active",
    m: { chat_asked_about_selection: () => "selection", chat_session_not_created: () => "missing", chat_message_not_sent: ({ error }) => `failed: ${error}`, chat_send_unconfirmed: ({ error }) => `unconfirmed: ${error}` },
    ltr: (value) => value, reseedSession: async () => {},
  };
  const send = new Function(...Object.keys(bindings), code)(...Object.values(bindings));
  return {
    send, bindings, stashes, mounted, actions, deleted, scope, key,
    get state() { return state; },
    render() { while (updates.length) updates.shift()(); composerLiveRef.current = state; },
    edit(draft) { state = { ...state, draft }; composerLiveRef.current = state; },
    switchAway() { scope.current = { projectId: "p", activeId: "other", mainView: "chat" }; },
    unmount() { bindings.composerMountedRef.current = false; },
    replaceWorkspace() { generation++; stashes.clear(); },
    reject(error = new Error("rejected")) { assert.ok(rejectSend); rejectSend(error); },
    resolve(value = {}) { assert.ok(resolveSend); resolveSend(value); },
  };
}
async function flush() { for (let i = 0; i < 15; i++) await Promise.resolve(); }

function mountComposer(s) {
  let effect;
  function visit(node) {
    if (ts.isCallExpression(node) && node.expression.getText(file) === "useLayoutEffect"
      && node.getText(file).includes("mountedComposers.set(stashKey")) effect = node.getText(file);
    ts.forEachChild(node, visit);
  }
  visit(file);
  assert.ok(effect);
  let cleanup;
  const bindings = {
    ...s.bindings, useLayoutEffect: (callback) => { cleanup = callback(); },
    composerPrefill: null,
  };
  const mount = ts.transpileModule(`${find("mergeFailedComposer")}\n${effect}`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  new Function(...Object.keys(bindings), mount)(...Object.values(bindings));
  return cleanup;
}

for (const busy of [false, true]) {
  for (const unmounted of [false, true]) {
    for (const newer of ["original", "newer"]) {
      test(`failed ${busy ? "queued" : "normal"} send preserves ${newer === "original" ? "identical" : "different"} later draft ${unmounted ? "after unmount" : "in the active chat"}`, async () => {
        const s = setup({ busy });
        s.stashes.set(s.key, s.state);
        const cleanup = mountComposer(s);
        s.render();
        const task = s.send();
        await flush();
        s.render();
        s.edit(newer);
        if (unmounted) { cleanup(); s.unmount(); }
        s.reject(new RejectedResponseError("rejected"));
        await task;
        assert.equal(unmounted ? s.stashes.get(s.key).draft : s.state.draft, `original\n\n${newer}`);
      });
    }
  }
}

test("failure before the clear renders restores one draft", async () => {
  const s = setup({ deferred: true, history: async () => { throw new Error("history unavailable"); } });
  s.stashes.set(s.key, s.state);
  mountComposer(s);
  s.render();
  await s.send();
  assert.equal(s.bindings.composerLiveRef.current.draft, "original");
  s.render();
  assert.equal(s.state.draft, "original");
});

test("unmount before the clear renders does not stash the sent draft", async () => {
  const s = setup({ deferred: true });
  s.stashes.set(s.key, s.state);
  const cleanup = mountComposer(s);
  s.render();
  const task = s.send();
  await flush();
  cleanup();
  s.unmount();
  s.reject(new RejectedResponseError("rejected"));
  await task;
  assert.equal(s.stashes.get(s.key).draft, "original");
});

test("failed plan toggle before the clear renders restores one draft", async () => {
  const s = setup({ deferred: true });
  s.bindings.parseComposerCommand = () => ({ name: "plan", prompt: "" });
  s.bindings.attachments = [];
  s.bindings.annotations = [];
  s.bindings.planActivation = "command";
  s.bindings.setIndependentPlanMode = async () => { throw new Error("toggle failed"); };
  s.bindings.setSettingsError = () => {};
  s.bindings.m.chat_toggle_plan_failed = () => "toggle failed";
  s.stashes.set(s.key, { ...s.state, attachments: [], annotations: [] });
  mountComposer(s);
  s.render();
  const send = new Function(...Object.keys(s.bindings), code)(...Object.values(s.bindings));
  await send();
  assert.equal(s.bindings.composerLiveRef.current.draft, "original");
  s.render();
  assert.equal(s.state.draft, "original");
});

test("failed plan toggle after unmount before the clear renders restores one draft", async () => {
  const s = setup({ deferred: true });
  let rejectToggle;
  s.bindings.parseComposerCommand = () => ({ name: "plan", prompt: "" });
  s.bindings.attachments = [];
  s.bindings.annotations = [];
  s.bindings.planActivation = "command";
  s.bindings.setIndependentPlanMode = () => new Promise((_, reject) => { rejectToggle = reject; });
  s.bindings.setSettingsError = () => {};
  s.bindings.m.chat_toggle_plan_failed = () => "toggle failed";
  s.stashes.set(s.key, { ...s.state, attachments: [], annotations: [] });
  const cleanup = mountComposer(s);
  s.render();
  const send = new Function(...Object.keys(s.bindings), code)(...Object.values(s.bindings));
  const task = send();
  cleanup();
  s.unmount();
  rejectToggle(new Error("toggle failed"));
  await task;
  assert.equal(s.stashes.get(s.key).draft, "original");
});

for (const side of [false, true]) {
  test(`failed ${side ? "side" : "main"} send after switch restores all content to its own stash`, async () => {
    const s = setup({ side });
    const task = s.send();
    await flush();
    s.switchAway();
    s.reject(new RejectedResponseError("rejected"));
    await task;
    assert.deepEqual(s.stashes.get(s.key), { draft: "original", attachments: s.bindings.attachments, annotations: s.bindings.annotations, unconfirmedSend: null, pendingClientTurn: null });
    assert.equal(s.actions.at(-1).sessionId, "s");
    assert.equal(s.state.draft, "");
  });
}

test("recovery merges newer content, including non-equivalent annotation IDs", async () => {
  const s = setup();
  const task = s.send();
  await flush();
  s.switchAway();
  const newer = { draft: "newer", attachments: [{ dataUrl: "data:,new" }], annotations: [{ id: "a", text: "new selection" }] };
  s.stashes.set(s.key, newer);
  s.reject(new RejectedResponseError("rejected"));
  await task;
  const restored = s.stashes.get(s.key);
  assert.equal(restored.draft, "original\n\nnewer");
  assert.deepEqual(restored.attachments, [...s.bindings.attachments, ...newer.attachments]);
  assert.deepEqual(restored.annotations.map((item) => item.text), ["old selection", "new selection"]);
  assert.notEqual(restored.annotations[0].id, restored.annotations[1].id);
});

test("a newly created session receives recovery after its original composer unmounts", async () => {
  const s = setup({ side: true, sessionId: null });
  const task = s.send();
  await flush();
  s.unmount();
  s.reject();
  await task;
  assert.equal(s.stashes.get("1:side:created").draft, "original");
  assert.equal(s.stashes.has(s.key), false);
});

for (const change of ["workspace", "deleted", "deleted parent"]) {
  test(`late failure cannot restore a ${change} chat`, async () => {
    const s = setup({ side: true });
    const task = s.send();
    await flush();
    s.switchAway();
    if (change === "workspace") s.replaceWorkspace();
    if (change === "deleted") s.deleted.add("s");
    if (change === "deleted parent") s.deleted.add("parent");
    s.reject();
    await task;
    assert.equal(s.stashes.size, 0);
  });
}

test("a successful send never restores its draft after switching", async () => {
  const s = setup();
  const task = s.send();
  await flush();
  s.switchAway();
  s.resolve();
  await task;
  assert.equal(s.stashes.size, 0);
});

test("a failed queued send restores its side composer and surfaces the error", async () => {
  const s = setup({ side: true, busy: true });
  const task = s.send();
  await flush();
  s.switchAway();
  s.reject(new RejectedResponseError("rejected"));
  await task;
  assert.equal(s.stashes.get(s.key).draft, "original");
  assert.equal(s.actions.at(-1).type, "localError");
});

for (const update of ["event", "refresh"]) {
  test(`an unconfirmed restored draft survives a server user message via ${update} and remount`, async (t) => {
    const s = setup();
    const task = s.send();
    await flush();
    s.unmount();
    s.reject(new TypeError("network"));
    await task;
    assert.equal(s.stashes.get(s.key)?.draft, "original");

    const serverMessage = { id: "server", role: "user", parts: [], createdAt: 1 };
    const snapshot = { messages: [serverMessage], queued: [], activeLeafId: "server" };
    const { client, load } = queryModules({ getChatMessages: async () => snapshot }, { nativeClient: true });
    t.after(() => client.clear());
    const options = load("chat").getChatMessagesQuery("s");
    client.setQueryData(options.queryKey, { messages: [], queued: [], activeLeafId: null });
    const store = load("chatStore");
    store.dispatchChat("p", { type: "localError", sessionId: "s", text: "transient" });
    if (update === "event") store.dispatchChat("p", { type: "upsertMessage", sessionId: "s", message: serverMessage }, true);
    else await client.fetchQuery({ ...options, staleTime: 0 });
    assert.deepEqual(client.getQueryData(options.queryKey).messages.map((message) => message.id), ["server"]);
    assert.equal(s.stashes.get(s.key)?.draft, "original");

    let effect;
    function visit(node) {
      if (ts.isCallExpression(node) && node.expression.getText(file) === "useLayoutEffect"
        && node.getText(file).includes("mountedComposers.set(stashKey")) effect = node.getText(file);
      ts.forEachChild(node, visit);
    }
    visit(file);
    assert.ok(effect);
    let current = { ...EMPTY_COMPOSER_STASH };
    let cleanup;
    const set = (field) => (value) => { current = { ...current, [field]: typeof value === "function" ? value(current[field]) : value }; };
    const live = { current };
    const bindings = {
      useLayoutEffect: (callback) => { cleanup = callback(); }, composerStashes: s.stashes, mountedComposers: s.mounted,
      stashKey: s.key, composerLiveRef: live, pendingClientTurn: { current: null }, pendingScope: { current: null }, EMPTY_COMPOSER_STASH, composerStashContent,
      isCurrentScope: () => true, sessionsOptions: { queryKey: [] }, activeId: "s", sideParentSessionId: null, deletedSessionIds: s.deleted,
      composerPrefill: null, setDraft: set("draft"), setAttachments: set("attachments"), setAnnotations: set("annotations"), setUnconfirmedSend: set("unconfirmedSend"), setImageRefusedFor: () => {}, crypto,
    };
    const mount = ts.transpileModule(`${find("mergeFailedComposer")}\n${effect}`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
    new Function(...Object.keys(bindings), mount)(...Object.values(bindings));
    assert.ok(cleanup);
    assert.equal(current.draft, "original");
    assert.equal(current.unconfirmedSend, "network");
    current = { ...current, draft: "edited", attachments: [], annotations: [] };
    live.current = current;
    cleanup();
    assert.equal(s.stashes.get(s.key).unconfirmedSend, "network");
    new Function(...Object.keys(bindings), mount)(...Object.values(bindings));
    assert.equal(current.draft, "edited");
    assert.equal(current.unconfirmedSend, "network");
    current = { ...current, unconfirmedSend: null };
    live.current = current;
    cleanup();
    assert.equal(s.stashes.get(s.key).unconfirmedSend, null);
  });
}

test("a failure after returning updates the mounted composer without losing newer edits", async () => {
  let effect;
  function visit(node) {
    if (ts.isCallExpression(node) && node.expression.getText(file) === "useLayoutEffect"
      && node.getText(file).includes("mountedComposers.set(stashKey")) effect = node.getText(file);
    ts.forEachChild(node, visit);
  }
  visit(file);
  assert.ok(effect);
  const s = setup();
  const task = s.send();
  await flush();
  let current = { draft: "newer", attachments: [{ dataUrl: "data:,new" }], annotations: [{ id: "b", text: "new selection" }] };
  const set = (field) => (value) => { current = { ...current, [field]: typeof value === "function" ? value(current[field]) : value }; };
  const bindings = {
    useLayoutEffect: (callback) => { callback(); }, composerStashes: s.stashes, mountedComposers: s.mounted,
    stashKey: s.key, composerLiveRef: { current: EMPTY_COMPOSER_STASH }, pendingClientTurn: { current: null }, pendingScope: { current: null }, EMPTY_COMPOSER_STASH, composerStashContent,
    isCurrentScope: () => true, sessionsOptions: { queryKey: [] }, activeId: "s", sideParentSessionId: null, deletedSessionIds: s.deleted,
    composerPrefill: null, setDraft: set("draft"), setAttachments: set("attachments"), setAnnotations: set("annotations"), setUnconfirmedSend: set("unconfirmedSend"), setImageRefusedFor: () => {}, crypto,
  };
  const mountCode = ts.transpileModule(`${find("mergeFailedComposer")}\n${effect}`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  new Function(...Object.keys(bindings), mountCode)(...Object.values(bindings));
  current = { draft: "newer", attachments: [{ dataUrl: "data:,new" }], annotations: [{ id: "b", text: "new selection" }] };
  s.reject();
  await task;
  assert.equal(current.draft, "original\n\nnewer");
  assert.equal(current.attachments.length, 2);
  assert.deepEqual(current.annotations.map((item) => item.text), ["old selection", "new selection"]);
});

for (const busy of [false, true]) {
  test(`a confirmed rejection after unmount restores content and reports rejection (${busy ? "queued" : "normal"})`, async () => {
    const s = setup({ busy });
    const task = s.send();
    await flush();
    s.unmount();
    s.reject(new RejectedResponseError("rejected"));
    await task;
    assert.equal(s.stashes.get(s.key).draft, "original");
    assert.equal(s.stashes.get(s.key).attachments.length, 1);
    assert.equal(s.stashes.get(s.key).annotations.length, 1);
    assert.equal(s.stashes.get(s.key).unconfirmedSend, null);
    assert.match(s.actions.at(-1).text, /^failed:/);
  });
}

for (const busy of [false, true]) {
  test(`an accepted ${busy ? "queued" : "normal"} send with a lost response stays cleared`, async () => {
    let lookup;
    const s = setup({ busy, submission: async (sid, id) => { lookup = [sid, id]; return { accepted: true }; } });
    const task = s.send();
    await flush();
    s.reject(new TypeError("network"));
    await task;
    assert.equal(lookup[0], "s");
    assert.match(lookup[1], /^ct_/);
    assert.equal(s.state.draft, "");
    assert.equal(s.state.unconfirmedSend, null);
    assert.equal(s.bindings.pendingClientTurn.current, null);
  });
}

test("an acceptance lookup cannot refresh a deleted session", async () => {
  let finishLookup, refreshed = false;
  const s = setup({ submission: () => new Promise((resolve) => { finishLookup = resolve; }) });
  s.bindings.reseedSession = async () => { refreshed = true; };
  const send = new Function(...Object.keys(s.bindings), code)(...Object.values(s.bindings));
  const task = send();
  await flush();
  s.reject(new TypeError("network"));
  await flush();
  s.deleted.add("s");
  finishLookup({ accepted: true });
  await task;
  assert.equal(refreshed, false);
  assert.equal(s.stashes.has(s.key), false);
});

test("a confirmed send after unmount clears its saved retry", async () => {
  const s = setup({ submission: async () => ({ accepted: true }) });
  s.stashes.set(s.key, s.state);
  const cleanup = mountComposer(s);
  s.render();
  const task = s.send();
  await flush();
  s.render();
  cleanup();
  s.unmount();
  s.reject(new TypeError("network"));
  await task;
  assert.equal(s.stashes.get(s.key)?.draft ?? "", "");
  assert.equal(s.stashes.get(s.key)?.pendingClientTurn ?? null, null);
});

test("a confirmed rejection does not query submission status", async () => {
  let lookedUp = false;
  const s = setup({ submission: async () => { lookedUp = true; return { accepted: true }; } });
  const task = s.send();
  await flush();
  s.reject(new RejectedResponseError("rejected"));
  await task;
  assert.equal(lookedUp, false);
});

for (const busy of [false, true]) {
  test(`unconfirmed ${busy ? "steering or queued" : "normal"} sends retain content and warn`, async () => {
    const s = setup({ busy, steering: busy });
    const task = s.send();
    await flush();
    s.unmount();
    s.reject(new TypeError("network"));
    await task;
    assert.equal(s.stashes.get(s.key).draft, "original");
    assert.equal(s.stashes.get(s.key).unconfirmedSend, "network");
    assert.equal(s.actions.some((action) => action.type === "localError"), false);
  });
}

test("a failed history read restores before any message is submitted", async () => {
  const s = setup({ history: async () => { throw new Error("history unavailable"); } });
  const task = s.send();
  s.switchAway();
  await task;
  assert.equal(s.stashes.get(s.key).draft, "original");
  assert.equal(s.actions.at(-1).type, "localError");
});

test("a session creation failure restores the unsaved composer and reports an error", async () => {
  const s = setup({ sessionId: null });
  const alerts = [];
  s.bindings.openNewSession = async () => { throw new Error("creation failed"); };
  s.bindings.showAlert = (message) => alerts.push(message);
  const send = new Function(...Object.keys(s.bindings), code)(...Object.values(s.bindings));
  const task = send();
  s.switchAway();
  await task;
  assert.equal(s.stashes.get(s.key).draft, "original");
  assert.match(alerts[0], /creation failed/);
});

test("an unchanged retry in the same composer retains its existing client ID", async () => {
  const s = setup();
  const task = s.send();
  await flush();
  const id = s.bindings.pendingClientTurn.current.id;
  s.reject(new TypeError("network"));
  await task;
  let retriedId;
  const bindings = { ...s.bindings, sendChatMessage: async (_sid, _text, _opts, _images, _annotations, clientId) => { retriedId = clientId; return {}; } };
  await new Function(...Object.keys(bindings), code)(...Object.values(bindings))();
  assert.equal(retriedId, id);
  assert.equal(s.state.unconfirmedSend, null);
});

test("sending a different message does not dismiss an earlier uncertain delivery", async () => {
  const s = setup();
  s.bindings.setUnconfirmedSend("previous response lost");
  s.render();
  const task = s.send();
  await flush();
  s.resolve();
  await task;
  assert.equal(s.state.unconfirmedSend, "previous response lost");
});

test("an accepted send in the active composer stays cleared", async () => {
  const s = setup();
  const task = s.send();
  await flush();
  s.resolve();
  await task;
  assert.equal(s.state.draft, "");
  assert.deepEqual(s.state.attachments, []);
  assert.deepEqual(s.state.annotations, []);
});

test("deletion during history refresh cannot recreate a local error", async () => {
  const s = setup({ side: true });
  let finishRefresh;
  s.bindings.reseedSession = () => new Promise((resolve) => { finishRefresh = resolve; });
  const send = new Function(...Object.keys(s.bindings), code)(...Object.values(s.bindings));
  const task = send();
  await flush();
  s.reject(new TypeError("network"));
  await flush();
  assert.ok(finishRefresh);
  s.deleted.add("parent");
  finishRefresh();
  await task;
  assert.equal(s.actions.some((action) => action.type === "localError"), false);
});

test("an uncertain send keeps its client ID after switching chats", async () => {
  const s = setup();
  let first;
  s.bindings.sendChatMessage = (...args) => { first = args[5]; return Promise.reject(new Error("lost response")); };
  mountComposer(s);
  s.render();
  await new Function(...Object.keys(s.bindings), code)(...Object.values(s.bindings))();
  s.render();
  const t = setup();
  t.stashes.set(t.key, { ...s.state, pendingClientTurn: s.bindings.pendingClientTurn.current });
  mountComposer(t);
  t.render();
  let retry;
  t.bindings.draft = t.state.draft;
  t.bindings.attachments = t.state.attachments;
  t.bindings.annotations = t.state.annotations;
  t.bindings.sendChatMessage = (...args) => { retry = args[5]; return Promise.resolve({}); };
  await new Function(...Object.keys(t.bindings), code)(...Object.values(t.bindings))();
  assert.equal(retry, first);
});

for (const steering of [false, true]) {
  test(`lost ${steering ? "steering" : "queued"} response preserves ID and delivery mode after a chat switch`, async () => {
    const original = setup({ busy: true, steering });
    let sent;
    original.bindings.sendChatMessage = (...args) => { sent = args; return Promise.reject(new Error("response lost")); };
    const cleanup = mountComposer(original);
    original.render();
    const first = new Function(...Object.keys(original.bindings), code)(...Object.values(original.bindings));
    const task = first();
    await flush();
    cleanup();
    original.unmount();
    await task;
    const restored = setup({ busy: false });
    restored.stashes.set(restored.key, original.stashes.get(original.key));
    mountComposer(restored);
    restored.render();
    Object.assign(restored.bindings, { draft: restored.state.draft, attachments: restored.state.attachments, annotations: restored.state.annotations });
    let retried;
    restored.bindings.sendChatMessage = (...args) => { retried = args; return Promise.resolve({}); };
    const retry = new Function(...Object.keys(restored.bindings), code)(...Object.values(restored.bindings));
    await retry();
    assert.equal(retried[5], sent[5]);
    assert.equal(retried[6], sent[6]);
    assert.deepEqual(retried[2], sent[2]);
    if (steering) assert.equal(restored.actions.some((action) => action.type === "optimisticUser"), false);
  });
}

test("editing a recovered draft creates a new client ID", async () => {
  const s = setup();
  let first;
  s.bindings.sendChatMessage = (...args) => { first = args[5]; return Promise.reject(new Error("response lost")); };
  mountComposer(s);
  s.render();
  await new Function(...Object.keys(s.bindings), code)(...Object.values(s.bindings))();
  s.render();
  s.edit("changed request");
  s.bindings.draft = s.state.draft;
  let next;
  s.bindings.sendChatMessage = (...args) => { next = args[5]; return Promise.resolve({}); };
  await new Function(...Object.keys(s.bindings), code)(...Object.values(s.bindings))();
  assert.notEqual(next, first);
  assert.equal(s.state.unconfirmedSend, "response lost");
});

test("a session settings refresh does not change the recovered request", async () => {
  const original = setup();
  original.bindings.composerSelection = { harness: "codex", model: "original-model", reasoningLevel: "high" };
  let first;
  original.bindings.sendChatMessage = (...args) => { first = args; return Promise.reject(new Error("response lost")); };
  const cleanup = mountComposer(original);
  original.render();
  const task = new Function(...Object.keys(original.bindings), code)(...Object.values(original.bindings))();
  await flush();
  cleanup();
  original.unmount();
  await task;
  const restored = setup();
  restored.stashes.set(restored.key, original.stashes.get(original.key));
  mountComposer(restored);
  restored.render();
  Object.assign(restored.bindings, { draft: restored.state.draft, attachments: restored.state.attachments, annotations: restored.state.annotations, composerSelection: { harness: "codex", model: "refreshed-model", reasoningLevel: "low" } });
  let second;
  restored.bindings.sendChatMessage = (...args) => { second = args; return Promise.resolve({}); };
  await new Function(...Object.keys(restored.bindings), code)(...Object.values(restored.bindings))();
  assert.equal(second[5], first[5]);
  assert.deepEqual(second[2], first[2]);
});

test("changing a picker invalidates a recovered retry but leaves its warning", () => {
  let invalidate, select;
  function visit(node) {
    if (ts.isVariableDeclaration(node) && node.name.getText(file) === "invalidatePendingRetry") invalidate = node.initializer.getText(file);
    if (ts.isVariableDeclaration(node) && node.name.getText(file) === "selectModel") select = node.initializer.getText(file);
    ts.forEachChild(node, visit);
  }
  visit(file);
  assert.ok(invalidate && select);
  const pending = { signature: "old", id: "ct-old", mode: null, options: { model: "old" } };
  const live = { current: { ...EMPTY_COMPOSER_STASH, unconfirmedSend: "response lost", pendingClientTurn: pending } };
  const ref = { current: pending };
  const bindings = {
    pendingClientTurn: ref, composerLiveRef: live, openSession: null, acpModel: null,
    composerSelection: { harness: "codex", model: "old" },
    setRecoveryOverrides: () => {}, setSelection: () => {}, onPreferredAgentChange: () => Promise.resolve(),
    setSessionOverride: () => {},
  };
  const js = ts.transpileModule(`const invalidatePendingRetry = ${invalidate};\nreturn ${select};`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  const choose = new Function(...Object.keys(bindings), js)(...Object.values(bindings));
  choose({ model: "old" });
  assert.equal(ref.current, pending);
  choose({ model: "new" });
  assert.equal(ref.current, null);
  assert.equal(live.current.pendingClientTurn, null);
  assert.equal(live.current.unconfirmedSend, "response lost");
});

test("a picker change while a request is in flight is not undone by a late failure", async () => {
  const s = setup();
  const cleanup = mountComposer(s);
  s.render();
  const task = s.send();
  await flush();
  const prior = s.bindings.pendingClientTurn.current;
  assert.ok(prior);
  prior.invalidated = true;
  s.bindings.pendingClientTurn.current = null;
  s.bindings.composerLiveRef.current = { ...s.bindings.composerLiveRef.current, pendingClientTurn: null };
  cleanup();
  s.unmount();
  s.reject(new Error("response lost"));
  await task;
  assert.equal(s.stashes.get(s.key).pendingClientTurn, null);
  assert.equal(s.stashes.get(s.key).unconfirmedSend, "response lost");
});

test("retrying a failed plan message does not toggle plan a second time", async () => {
  const original = setup();
  original.bindings.parseComposerCommand = () => ({ name: "plan", prompt: "original" });
  original.bindings.planActivation = "command";
  original.bindings.setPlanModeOverride = () => {};
  let first;
  original.bindings.sendChatMessage = (...args) => { first = args; return Promise.reject(new Error("response lost")); };
  const cleanup = mountComposer(original);
  original.render();
  const task = new Function(...Object.keys(original.bindings), code)(...Object.values(original.bindings))();
  await flush();
  cleanup();
  original.unmount();
  await task;
  const restored = setup();
  restored.stashes.set(restored.key, original.stashes.get(original.key));
  restored.bindings.parseComposerCommand = () => ({ name: "plan", prompt: "original" });
  restored.bindings.planActivation = "command";
  restored.bindings.setPlanModeOverride = () => assert.fail("retry changed plan mode");
  mountComposer(restored);
  restored.render();
  Object.assign(restored.bindings, { draft: restored.state.draft, attachments: restored.state.attachments, annotations: restored.state.annotations });
  let second;
  restored.bindings.sendChatMessage = (...args) => { second = args; return Promise.resolve({}); };
  await new Function(...Object.keys(restored.bindings), code)(...Object.values(restored.bindings))();
  assert.equal(second[5], first[5]);
  assert.deepEqual(second[2], first[2]);
});

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";

const source = (file) => ts.createSourceFile(file, readFileSync(new URL(`../src/components/${file}`, import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
function evaluate(code, bindings) {
  const js = ts.transpileModule(code, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  return new Function(...Object.keys(bindings), js)(...Object.values(bindings));
}
function declaration(file, name) {
  let found;
  function visit(node) {
    if (ts.isVariableDeclaration(node) && node.name.getText(file) === name) found = node;
    ts.forEachChild(node, visit);
  }
  visit(file);
  assert.ok(found, name);
  return found.initializer.getText(file);
}

function sendPreparation() {
  const file = source("ChatPanel.tsx");
  let send;
  function visit(node) {
    if (ts.isFunctionDeclaration(node) && node.name?.text === "send") send = node;
    ts.forEachChild(node, visit);
  }
  visit(file);
  const turn = send.body.statements.find((node) => ts.isTryStatement(node) && node.tryBlock.getText(file).includes('type: "optimisticUser"'));
  const statements = turn.tryBlock.statements;
  const optimisticIndex = statements.findIndex((node) => node.getText(file).includes('type: "optimisticUser"'));
  const prefix = statements.slice(0, optimisticIndex + 1).map((node) => node.getText(file)).join("\n");
  return prefix;
}

for (const [name, fields] of [["K8s", ["context", "namespace"]], ["Slurm", ["host", "partition", "account", "timeLimit"]], ["Ray", ["address"]]]) {
  test(`${name} settings refresh clean fields while preserving edits`, () => {
    const file = source("SettingsPage.tsx");
    const component = file.statements.find((node) => ts.isFunctionDeclaration(node) && node.name.text === `${name}Section`);
    const statements = component.body.statements;
    const prefix = statements.slice(0, statements.findIndex(ts.isReturnStatement)).map((node) => node.getText(file)).join("\n");
    const state = [];
    let cursor, effects, changed;
    let snapshot = Object.fromEntries(fields.map((field) => [field, "original"]));
    const hook = (init) => { const index = cursor++; if (!(index in state)) state[index] = init(); return index; };
    const bindings = {
      useState: (initial) => { const i = hook(() => initial); return [state[i], (next) => { const value = typeof next === "function" ? next(state[i]) : next; changed ||= !Object.is(value, state[i]); state[i] = value; }]; },
      useRef: (initial) => state[hook(() => ({ current: initial }))],
      useEffect: (effect, deps) => { const i = hook(() => undefined); if (!state[i] || deps.some((value, index) => !Object.is(value, state[i][index]))) { state[i] = deps; effects.push(effect); } },
      useMutation: () => ({}), useQuery: () => ({ data: snapshot }),
      useSshMasterStatuses: () => [{}, () => {}],
      [`get${name}SettingsQuery`]: () => ({ queryKey: [] }), [`save${name}Settings`]: () => {},
      remote: false, onEditState: undefined,
    };
    const code = `${prefix}\nreturn { values: {${fields}}, setters: {${fields.map((field) => `${field}: set${field[0].toUpperCase()}${field.slice(1)}`).join(",")}} };`;
    function render() {
      for (let attempt = 0; attempt < 5; attempt++) {
        cursor = 0; effects = []; changed = false;
        const result = evaluate(code, bindings);
        effects.forEach((effect) => effect());
        if (!changed) return result;
      }
      assert.fail("settings effects did not settle");
    }
    assert.deepEqual(render().values, snapshot);
    snapshot = Object.fromEntries(fields.map((field) => [field, "refreshed"]));
    assert.deepEqual(render().values, snapshot);
    render().setters[fields[0]]("my edit");
    const edited = render().values;
    snapshot = Object.fromEntries(fields.map((field) => [field, "external"]));
    assert.deepEqual(render().values, edited);
  });
}

test("queued chat writes stop when their captured workspace retires", async () => {
  const file = source("ChatPanel.tsx");
  let generation = 1, release, calls = 0;
  const queue = evaluate(`return ${declaration(file, "queueSessionMutation")}`, {
    useCallback: (fn) => fn, sessionsOptions: { queryKey: ["workspace", 1] },
    settingsMutationTail: { current: Promise.resolve() }, isCurrentScope: (key) => key[1] === generation,
  });
  const first = queue(() => new Promise((resolve) => { release = resolve; calls++; }));
  const second = queue(async () => { calls++; });
  const rejected = assert.rejects(second, { name: "AbortError" });
  for (let i = 0; i < 5; i++) await Promise.resolve();
  generation++;
  release();
  await first;
  await rejected;
  assert.equal(calls, 1);
});

test("old prompt responses do not reconcile against the replacement workspace", async () => {
  const file = source("ChatPanel.tsx");
  let generation = 1, release, reads = 0;
  const respond = evaluate(`return ${declaration(file, "respond")}`, {
    useCallback: (fn) => fn, activeId: "old-session", projectId: "old-project", dispatch: () => {},
    sessionsOptions: { queryKey: ["workspace", 1] }, isCurrentScope: (key) => key[1] === generation,
    queueSessionMutation: () => new Promise((resolve) => { release = resolve; }),
    queryClient: { fetchQuery: async () => { reads++; } },
    getChatMessagesQuery: () => ({}), listChatSessionsQuery: () => ({}),
  });
  const answer = respond({});
  generation++;
  release();
  await answer;
  assert.equal(reads, 0);
  generation = 1;
  const current = respond({});
  release();
  await current;
  assert.equal(reads, 2);
});

for (const [card, family, field, upload] of [
  ["SkillsCard", "listUserSkills", "skills", "uploadUserSkill"],
  ["LatexTemplatesCard", "listLatexTemplates", "templates", "uploadLatexTemplate"],
]) {
  test(`${card} retains cached rows when refresh fails`, async (t) => {
    const { QueryObserver } = await import("@tanstack/react-query");
    const { queryModules } = await import("./queryModules.mjs");
    let fail = false, calls = 0;
    const { client, load } = queryModules({ [family]: async () => { calls++; if (fail) throw new Error("offline"); return family === "listUserSkills" ? { skills: [{ name: "cached" }], importing: false } : [{ name: "cached" }]; } }, { nativeClient: true });
    const options = load("settings")[`${family}Query`]();
    await client.fetchQuery(options);
    const observer = new QueryObserver(client, options);
    const off = observer.subscribe(() => {});
    t.after(() => { off(); client.clear(); });
    const file = source("SkillsTab.tsx");
    const component = file.statements.find((node) => ts.isFunctionDeclaration(node) && node.name.text === card);
    const statements = component.body.statements;
    const prefix = statements.slice(0, statements.findIndex(ts.isReturnStatement)).map((node) => node.getText(file)).join("\n");
    const render = () => evaluate(`${prefix}\nreturn { rows: ${field}, loadError, refresh: ${card === "SkillsCard" ? "refresh" : "() => templatesQuery.refetch()"} };`, {
      useQuery: () => observer.getCurrentResult(), useMutation: () => ({}),
      useLayoutEffect: () => {}, useState: (value) => [value, () => {}], useCallback: (fn) => fn, useRef: (current) => ({ current }),
      [`${family}Query`]: () => options, [upload]: () => {},
    });
    assert.equal(render().rows[0].name, "cached");
    fail = true;
    render().refresh();
    for (let i = 0; i < 30; i++) await Promise.resolve();
    assert.equal(calls, 2);
    assert.equal(render().rows[0].name, "cached");
    assert.equal(render().loadError, "offline");
  });
}

test("first send waits for cold history so an accepted reply replaces its optimistic branch", async (t) => {
  const { queryModules } = await import("./queryModules.mjs");
  let resolveHistory;
  let snapshot;
  const cold = new Promise((resolve) => { resolveHistory = resolve; });
  const { client, load } = queryModules({ getChatMessages: () => snapshot ? Promise.resolve(snapshot) : cold }, { nativeClient: true });
  t.after(() => client.clear());
  const options = load("chat").getChatMessagesQuery("s");
  const read = client.fetchQuery(options);
  const prefix = sendPreparation();
  const sending = evaluate(`return (async () => { let sid = "s"; ${prefix} })()`, {
    sessionsOptions: { queryKey: [] }, isCurrentScope: () => true,
    preparingSend: { current: false }, inSourceScope: () => false,
    queryClient: client, getChatMessagesQuery: load("chat").getChatMessagesQuery,
    dispatch: (action) => load("chatStore").dispatchChat("p", action),
    text: "new turn", pending: [], pendingAnnotations: [],
  });
  assert.equal(client.getQueryData(options.queryKey), undefined);
  snapshot = { messages: [{ id: "history", role: "user", parts: [], createdAt: 1 }], queued: [], activeLeafId: "history" };
  resolveHistory(snapshot);
  await read;
  await sending;
  assert.match(client.getQueryData(options.queryKey).activeLeafId, /^local-/);
  snapshot = { messages: [...snapshot.messages,
    { id: "accepted", role: "user", parts: [], createdAt: 2, parentId: "history" },
    { id: "reply", role: "assistant", parts: [], createdAt: 3, parentId: "accepted" },
  ], queued: [], activeLeafId: "reply" };
  const reconciled = await client.fetchQuery({ ...options, staleTime: 0 });
  assert.deepEqual(reconciled.messages.map((message) => message.id), ["history", "accepted", "reply"]);
  assert.equal(reconciled.activeLeafId, "reply");
});

test("navigation during history preparation preserves the unsent composer", async (t) => {
  const { QueryObserver } = await import("@tanstack/react-query");
  const { queryModules } = await import("./queryModules.mjs");
  const { client, load } = queryModules({ getChatMessages: () => new Promise(() => {}) }, { nativeClient: true });
  t.after(() => client.clear());
  const observer = new QueryObserver(client, load("chat").getChatMessagesQuery("s"));
  const off = observer.subscribe(() => {});
  const preparingSend = { current: false };
  let cleared = false;
  const sending = evaluate(`return (async () => { let sid = "s"; ${sendPreparation()} })()`, {
    sessionsOptions: { queryKey: [] }, isCurrentScope: () => true, preparingSend,
    queryClient: client, getChatMessagesQuery: load("chat").getChatMessagesQuery,
    inSourceScope: () => true,
    setDraft: () => { cleared = true; }, setAttachments: () => { cleared = true; },
    setAnnotations: () => { cleared = true; }, setAttachError: () => {},
    dispatch: () => assert.fail("must not send before history"),
  });
  const rejected = assert.rejects(sending);
  off();
  await rejected;
  assert.equal(cleared, false);
  assert.equal(preparingSend.current, false);
});

test("send joins a cold repair despite its intermediate cached history", async (t) => {
  const { queryModules } = await import("./queryModules.mjs");
  let first, repair;
  const initial = new Promise((resolve) => { first = resolve; });
  const repairing = new Promise((resolve) => { repair = resolve; });
  let calls = 0;
  const { client, load } = queryModules({ getChatMessages: () => ++calls === 1 ? initial : repairing }, { nativeClient: true });
  t.after(() => client.clear());
  const options = load("chat").getChatMessagesQuery("s");
  const read = client.fetchQuery(options);
  load("live").markLiveUpdate(client, options.queryKey);
  const snapshot = { messages: [{ id: "history", role: "user", parts: [], createdAt: 1 }], queued: [], activeLeafId: "history" };
  first(snapshot);
  for (let i = 0; i < 20; i++) await Promise.resolve();
  assert.deepEqual(client.getQueryData(options.queryKey), snapshot);
  assert.equal(client.getQueryState(options.queryKey).fetchStatus, "fetching");
  let dispatched = false;
  const sending = evaluate(`return (async () => { let sid = "s"; ${sendPreparation()} })()`, {
    sessionsOptions: { queryKey: [] }, isCurrentScope: () => true,
    preparingSend: { current: false }, inSourceScope: () => false,
    queryClient: client, getChatMessagesQuery: load("chat").getChatMessagesQuery,
    dispatch: () => { dispatched = true; }, text: "new", pending: [], pendingAnnotations: [],
  });
  for (let i = 0; i < 20; i++) await Promise.resolve();
  assert.equal(dispatched, false);
  repair(snapshot);
  await read;
  await sending;
  assert.equal(dispatched, true);
  assert.equal(calls, 2);
});

test("send clears the unchanged raw draft but preserves edits during preparation", async () => {
  for (const edited of [false, true]) {
    const draft = "hello\n";
    let current = draft;
    const sending = evaluate(`return (async () => { let sid = "s"; ${sendPreparation()} })()`, {
      sessionsOptions: { queryKey: [] }, isCurrentScope: () => true,
      preparingSend: { current: false }, inSourceScope: () => true,
      queryClient: { fetchQuery: async () => ({}), getQueryState: () => undefined },
      getChatMessagesQuery: () => ({ queryKey: [] }),
      draft, originalText: draft.trim(), text: draft.trim(), pending: [], pendingAnnotations: [],
      setDraft: (update) => { current = update(current); },
      setAttachments: () => {}, setAnnotations: () => {}, setAttachError: () => {}, dispatch: () => {},
    });
    if (edited) current = "next message";
    await sending;
    assert.equal(current, edited ? "next message" : "");
  }
});

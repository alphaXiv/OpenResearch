import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import React from "react";
import * as jsx from "react/jsx-runtime";
import ts from "typescript";

class AlphaXivNotConnectedError extends Error {}

const code = ts.transpileModule(readFileSync(new URL("../src/components/Onboarding.tsx", import.meta.url), "utf8"), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;

function setup({ datacenter = true, lookupFails = false, delayed = false, remote = true, harnesses, user = null, alphaXivError = null } = {}) {
  const values = [];
  const effects = [];
  const dialogs = [];
  const writes = [];
  const done = [];
  let cursor = 0;
  let releaseNetwork;
  const screen = { isConnected: true };
  const ready = { id: "codex", name: "Codex", installed: true, agentReady: true, models: [], options: {} };
  const claude = { id: "claude-code", name: "Claude Code", installed: false, agentReady: false, models: [], options: {} };
  const data = { harnesses: harnesses ?? [ready, claude], path: { gitVersion: "git 2.49.0" }, commands: { "claude-code": { install: "install", login: "login" } }, network: lookupFails ? undefined : { ip: "192.0.2.1", isDatacenter: datacenter } };
  const queries = Object.fromEntries([
    ["getHarnessesQuery", "harnesses"], ["getProjectPathStatusQuery", "path"], ["getHarnessSetupCommandsQuery", "commands"], ["getPublicNetworkQuery", "network"],
  ].map(([name, kind]) => [name, () => ({ kind })]));
  queries.refreshHarnesses = async () => data.harnesses;
  const workspace = {
    api: {
      captureUiEvent: () => { if (remote) assert.fail("remote setup is not the onboarding funnel"); },
      completeOnboarding: async (selection) => {
        if (remote) assert.fail("remote setup must not seed a demo");
        return { project: { id: "demo" }, selection };
      },
      updateUiState: async (body) => { writes.push(body); return { preferredAgent: body.preferredAgent }; },
    },
    queries,
    dialogs: { showCustomDialog: (dialog) => dialogs.push(dialog) },
    queryClient: {
      fetchQuery: async ({ kind }) => {
        if (remote) assert.notEqual(kind, "path", "remote agent setup does not run the local Git setup");
        if (kind === "network" && delayed) return new Promise((resolve) => { releaseNetwork = () => resolve(data.network); });
        if (kind === "network" && lookupFails) throw new Error("lookup failed");
        return data[kind];
      },
      getQueryData: () => data.harnesses,
    },
    workspaceScope: () => ["workspace", "remote"],
    isCurrentScope: () => true,
    socketUrl: (path) => new URL(path, "ws://localhost"),
  };
  const react = {
    ...React,
    useContext: () => ({ id: (id) => id }),
    useState: (initial) => {
      const index = cursor++;
      if (!(index in values)) values[index] = typeof initial === "function" ? initial() : initial;
      return [values[index], (value) => { values[index] = typeof value === "function" ? value(values[index]) : value; }];
    },
    useRef: (initial) => {
      const index = cursor++;
      return values[index] ??= { current: initial };
    },
    useEffect: (effect) => effects.push(effect),
  };
  const messages = new Proxy({}, { get: (_, name) => () => String(name) });
  const fallback = new Proxy({}, { get: (_, name) => name });
  const modules = {
    react, "react/jsx-runtime": jsx,
    "../workspace": { useWorkspace: () => workspace },
    "../api": { reasoningFor: () => ({ defaultId: null }), AlphaXivNotConnectedError },
    "../machineSelection": { useMachines: () => ({ owner: { queries: {
      useOpenResearchAccount: () => ({ data: { user }, isLoading: false }),
      useAlphaXivMe: () => ({ data: alphaXivError ? undefined : { avatarUrl: null }, error: alphaXivError, isSuccess: !alphaXivError, isLoading: false }),
    } } }) },
    "../i18n": { ltr: String },
    "../paraglide/messages.js": { m: messages },
    "@tanstack/react-query": {
      useQuery: ({ kind }) => ({ data: data[kind], isError: false }),
      useMutation: ({ mutationFn }) => ({ mutateAsync: mutationFn }),
    },
  };
  const exports = {};
  new Function("require", "exports", code)((name) => modules[name] ?? fallback, exports);
  const nodes = (node) => React.isValidElement(node) ? [node, ...React.Children.toArray(node.props.children).flatMap(nodes)] : [];
  const render = () => {
    cursor = 0;
    effects.length = 0;
    const root = exports.Onboarding(remote
      ? { remote: true, host: "gpu-lab", preferredAgent: null, onDone: (state) => done.push(state) }
      : { remote: false, preferredAgent: null, onDone: (project) => done.push(project) });
    root.props.ref.current = screen;
    return nodes(root);
  };
  render();
  effects.splice(0).forEach((effect) => effect());
  const app = {
    render, dialogs, writes, done,
    leave: () => { screen.isConnected = false; },
    release: () => releaseNetwork(),
    signIn: async () => {
      claude.installed = true;
      claude.authState = "needsLogin";
      claude.loginEligible = true;
      render().find((node) => node.props.h?.id === "claude-code").props.onSetup();
      await new Promise(setImmediate);
    },
    install: async () => {
      render().find((node) => node.props.h?.id === "claude-code").props.onSetup();
      await new Promise(setImmediate);
    },
  };
  return app;
}

test("remote setup starts at agents and saves only the machine's preferred agent", async () => {
  const app = setup();
  await new Promise(setImmediate);
  const tree = app.render();
  const text = tree.flatMap((node) => React.Children.toArray(node.props.children)).filter((child) => typeof child === "string");
  assert.ok(text.includes("remote_agents_title"));
  assert.ok(text.includes("remote_agents_description"));
  assert.ok(!text.includes("onboarding_step_1_of_2"));
  assert.ok(!text.includes("onboarding_back"));
  assert.ok(!text.includes("onboarding_alphaxiv_title"));
  const next = tree.find((node) => node.type === "Button" && React.Children.toArray(node.props.children).includes("onboarding_continue"));
  assert.equal(next.props.disabled, false);
  await next.props.onClick();
  await new Promise(setImmediate);
  assert.equal(app.writes.length, 1);
  assert.deepEqual(Object.keys(app.writes[0]), ["preferredAgent"]);
  assert.equal(app.writes[0].preferredAgent.harness, "codex");
  assert.equal(app.done.length, 1);
});

test("a datacenter warning precedes Claude setup once across install and sign in", async () => {
  const app = setup();
  await app.install();
  assert.equal(app.dialogs.length, 1);
  assert.equal(app.dialogs[0].component.name, "ClaudeDatacenterWarning");
  assert.deepEqual(app.dialogs[0].props, { host: "gpu-lab", ip: "192.0.2.1" });
  app.dialogs[0].onConfirm();
  assert.equal(app.dialogs.length, 2);
  await app.signIn();
  assert.equal(app.dialogs.length, 3);
  assert.equal(app.dialogs[2].props.harness.authState, "needsLogin");
  assert.equal(app.dialogs.filter((dialog) => dialog.component.name === "ClaudeDatacenterWarning").length, 1);
});

test("dismissing the warning does not repeat it on the next attempt", async () => {
  const app = setup();
  await app.install();
  assert.equal(app.dialogs[0].onCancel, "close");
  await app.install();
  assert.equal(app.dialogs.length, 2);
  assert.equal(app.dialogs.filter((dialog) => dialog.component.name === "ClaudeDatacenterWarning").length, 1);
});

for (const options of [{ datacenter: false }, { datacenter: null }, { lookupFails: true }]) {
  test(`Claude setup remains available without a known datacenter verdict: ${JSON.stringify(options)}`, async () => {
    const app = setup(options);
    await app.install();
    assert.equal(app.dialogs.length, 1);
    assert.notEqual(app.dialogs[0].component.name, "ClaudeDatacenterWarning");
  });
}


test("leaving agent setup cancels a delayed Claude install prompt", async () => {
  const app = setup({ delayed: true });
  await app.install();
  assert.equal(app.dialogs.length, 0);
  app.leave();
  app.release();
  await new Promise(setImmediate);
  assert.equal(app.dialogs.length, 0);
});

for (const [name, options, label] of [
  ["signed out", {}, "onboarding_skip"],
  ["signed in to alphaXiv", { user: { email: "a@b.c" } }, "onboarding_get_started"],
  ["signed in without an alphaXiv connection", { user: { email: "a@b.c" }, alphaXivError: new AlphaXivNotConnectedError() }, "onboarding_skip"],
]) {
  test(`the alphaXiv step offers to finish by skipping only until alphaXiv is linked: ${name}`, async () => {
    const app = setup({ remote: false, ...options });
    await new Promise(setImmediate);
    const button = (text) => app.render().find((node) => node.type === "Button" && React.Children.toArray(node.props.children).includes(text));
    button("onboarding_continue").props.onClick();
    await button("onboarding_continue").props.onClick();
    const tree = app.render();
    const card = tree.find((node) => typeof node.type === "function" && node.type.name === "AlphaXivAccountCard");
    assert.equal(card.props.connectOnly, !!options.alphaXivError);
    assert.equal(card.props.user, label === "onboarding_get_started" ? options.user : null);
    assert.ok(tree.some((node) => React.Children.toArray(node.props.children).includes(label)));
  });
}

test("Continue explains when the chosen agent is still loading", async () => {
  const pending = { id: "codex", name: "Codex", installed: true, agentReady: true, catalogPending: true, models: [], options: {} };
  const ready = { id: "opencode", name: "OpenCode", installed: true, agentReady: true, models: [], options: {} };
  const app = setup({ harnesses: [pending, ready] });
  await new Promise(setImmediate);
  app.render().find((node) => node.props.h?.id === "codex").props.onSelect();
  const next = app.render().find((node) => node.type === "Button" && React.Children.toArray(node.props.children).includes("onboarding_continue"));
  await next.props.onClick();
  const alert = app.render().find((node) => node.props.role === "alert");
  assert.equal(alert?.props.children, "harness_setup_not_ready");
  assert.equal(app.writes.length, 0);
});

test("finishing alphaXiv sign-in completes onboarding", async () => {
  const app = setup({ remote: false });
  await new Promise(setImmediate);
  const button = (text) => app.render().find((node) => node.type === "Button" && React.Children.toArray(node.props.children).includes(text));
  button("onboarding_continue").props.onClick();
  await button("onboarding_continue").props.onClick();
  const card = app.render().find((node) => typeof node.type === "function" && node.type.name === "AlphaXivAccountCard");
  card.props.onSignedIn();
  await new Promise(setImmediate);
  assert.deepEqual(app.done, [{ id: "demo" }]);
});

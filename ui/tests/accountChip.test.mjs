import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import ts from "typescript";

const require = createRequire(import.meta.url);
const code = ts.transpileModule(readFileSync(new URL("../src/components/AccountChip.tsx", import.meta.url), "utf8"), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;

class NotConnected extends Error {}
class AccountError extends Error { constructor(kind) { super(kind); this.kind = kind; } }
function setup(query, folderError = null) {
  const stores = new Map();
  let values = [];
  const posted = [];
  const dialogs = [];
  let cursor = 0;
  let signedOut = 0;
  let invalidated = 0;
  let confirm;
  const menu = { Root: "menu-root", Trigger: "menu-trigger", Portal: "portal", Positioner: "positioner", Popup: "menu-popup", Item: "menu-item" };
  const mocks = {
    react: { ...React, useEffect() {}, useRef: () => ({ current: null }), useState: (initial) => {
      const store = values;
      const index = cursor++;
      if (!(index in store)) store[index] = initial;
      return [store[index], (value) => { store[index] = typeof value === "function" ? value(store[index]) : value; }];
    } },
    "@tanstack/react-query": { useQuery: () => ({ data: null }) },
    "../api": { AlphaXivNotConnectedError: NotConnected, OpenResearchAccountError: AccountError },
    "@base-ui/react/menu": { Menu: menu },
    "../dialog": { showCustomDialog: (options) => { dialogs.push(options); } },
    "../queries/settings": {
      getRuntimeQuery: () => ({ queryKey: ["gateway", "runtime"] }),
      useOpenResearchAccount: () => query,
      useAlphaXivFolders: () => ({ error: folderError }),
      useSignOutOpenResearch: () => ({ isPending: false, mutateAsync: async () => { signedOut++; } }),
    },
    "../queries/invalidation": { invalidateFamilies: () => { invalidated++; } },
    "../queries/client": { queryClient: { invalidateQueries: async () => {}, getQueryData: () => null }, workspaceKey: (key) => [key] },
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
    "./confirmOpenResearchSignOut": { confirmOpenResearchSignOut: (onConfirm) => { confirm = onConfirm; } },
    "./OpenResearchAccount": { OpenResearchAccount: "account", accountErrorMessage: (error) => error ? "outage reason" : null },
    "./ui": { Button: "button", DialogContent: "dialog-content", DialogTitle: "dialog-title", MenuItem: "button", Spinner: "spinner", showAlert: () => {} },
  };
  const exports = {};
  new Function("require", "exports", "window", code)((name) => mocks[name] ?? require(name), exports, { ipc: { postMessage: (message) => posted.push(message) } });
  const nodes = (tree) => React.isValidElement(tree) ? [tree, ...React.Children.toArray(tree.props.children).flatMap(nodes)] : [];
  const renderIn = (key, component) => {
    if (!stores.has(key)) stores.set(key, []);
    values = stores.get(key);
    cursor = 0;
    return nodes(component());
  };
  const render = () => renderIn("chip", () => exports.AccountChip({ onDetails() {} }));
  /** The latest sign-in dialog's content; its `confirm` and `cancel` calls land in `calls`. */
  const dialog = (calls = []) => {
    const options = dialogs.at(-1);
    return renderIn(options, () => options.component({ ...options.props, confirm: () => calls.push("confirm"), cancel: () => calls.push("cancel"), cancelable: true, isDisabled: false, isPending: false, errorMessage: null }));
  };
  return { render, dialog, dialogs, posted, openSignIn: exports.openSignIn, confirm: () => confirm(), counts: () => [signedOut, invalidated] };
}

const user = { id: "u", firstName: "Clover", lastName: null, email: "c@example.org", alphaxivUrl: "https://www.alphaxiv.org" };
test("account menu shows identity, account details and sign out; captions name the account's provider", async () => {
  for (const linked of [true, false]) {
    const app = setup({ data: { loggedIn: true, apiUrl: "https://api.example.org", user: { ...user, alphaxivUrl: linked ? user.alphaxivUrl : null } } });
    const tree = app.render();
    assert.ok(tree.some((node) => node.type === "menu-trigger"));
    assert.equal(tree.find((node) => node.type === "menu-root").props.loopFocus, true);
    assert.ok(tree.some((node) => node.type === "account" && node.props.user.email === user.email));
    assert.ok(tree.some((node) => node.props.children === (linked ? "alphaXiv" : "OpenResearch")));
    assert.deepEqual(tree.filter((node) => node.type === "menu-item").map((node) => node.props.children), ["account_details", "settings_openresearch_sign_out"]);
    tree.find((node) => node.type === "menu-item" && node.props.children === "settings_openresearch_sign_out").props.onClick();
    assert.deepEqual(app.counts(), [0, 0]);
    await app.confirm();
    assert.deepEqual(app.counts(), [1, 0]);
  }
});

test("signed out opens the sign-in dialog; cancel rechecks the account", () => {
  const app = setup({ data: { loggedIn: false, user: null }, isLoading: false, isError: false });
  app.render().find((node) => node.type === "button").props.onClick();
  assert.equal(app.dialogs.length, 1);
  assert.equal(app.dialogs[0].props.connectOnly, false);
  const calls = [];
  app.dialog(calls).find((node) => node.type === "button" && node.props.children === "settings_page_cancel").props.onClick();
  assert.deepEqual(calls, ["cancel"]);
  app.dialogs[0].onCancel();
  assert.deepEqual(app.counts(), [0, 1]);
});

test("unavailable account retries instead of presenting signed out", () => {
  let retries = 0;
  const app = setup({ error: new AccountError("offline"), isError: true, isLoading: false, refetch: () => { retries++; } });
  const tree = app.render();
  assert.ok(tree.some((node) => node.props.children === "outage reason"));
  tree.find((node) => node.type === "menu-item" && node.props.children === "settings_check_again").props.onClick();
  assert.equal(retries, 1);
});


test("account menu starts the alphaXiv-only connection when absent", () => {
  const app = setup({ data: { loggedIn: true, apiUrl: "https://api.example.org", user } }, new NotConnected());
  const item = app.render().find((node) => node.type === "menu-item" && node.props.children === "library_connect");
  assert.ok(item);
  item.props.onClick();
  const session = app.dialog().find((node) => typeof node.type === "function" && node.type.name === "LoginSession");
  assert.equal(session.props.connectOnly, true);
});

test("finished sign-in closes the dialog and brings the desktop window forward", () => {
  const app = setup({ data: { loggedIn: false, user: null }, isLoading: false, isError: false });
  app.render().find((node) => node.type === "button").props.onClick();
  const calls = [];
  app.dialog(calls).find((node) => typeof node.type === "function" && node.type.name === "LoginSession").props.onComplete();
  assert.deepEqual(calls, ["confirm"]);
  app.dialogs[0].onConfirm();
  assert.deepEqual(app.posted, ["window:focus"]);
  assert.deepEqual(app.counts(), [0, 1]);
});

test("failed connection offers a fresh retry", () => {
  const app = setup({ data: { loggedIn: true, user } }, new NotConnected());
  app.render().find((node) => node.type === "menu-item" && node.props.children === "library_connect").props.onClick();
  const session = app.dialog().find((node) => typeof node.type === "function" && node.type.name === "LoginSession");
  session.props.onError(1);
  const tree = app.dialog();
  assert.ok(tree.some((node) => node.props.role === "alert" && node.props.children === "account_alphaxiv_not_connected"));
  tree.find((node) => node.type === "button" && node.props.children === "app_retry").props.onClick();
  assert.notEqual(app.dialog().find((node) => typeof node.type === "function" && node.type.name === "LoginSession").key, session.key);
});

test("OpenResearch failing to reach alphaXiv asks to try again in a moment", () => {
  for (const connectOnly of [true, false]) {
    const app = connectOnly ? setup({ data: { loggedIn: true, user } }, new NotConnected()) : setup({ data: { loggedIn: false, user: null }, isLoading: false, isError: false });
    app.render().find((node) => connectOnly ? node.type === "menu-item" && node.props.children === "library_connect" : node.type === "button").props.onClick();
    app.dialog().find((node) => typeof node.type === "function" && node.type.name === "LoginSession").props.onError(75);
    assert.ok(app.dialog().some((node) => node.props.role === "alert" && node.props.children === "account_alphaxiv_unavailable"));
  }
});

test("an account outage hides the alphaXiv connection action even with cached identity", () => {
  const app = setup({ data: { loggedIn: true, user }, error: new AccountError("offline"), isError: true }, new NotConnected());
  assert.ok(!app.render().some((node) => node.props.children === "library_connect"));
});

test("sign-in started elsewhere returns focus to the control that started it", () => {
  const app = setup({ data: { loggedIn: false, user: null }, isLoading: false, isError: false });
  const opener = { focus() {} };
  app.openSignIn(false, opener);
  assert.equal(app.dialog().find((node) => node.type === "dialog-content").props.finalFocus(), opener);
});

test("LoginSession navigates placeholder tab on remote sessions without opening a duplicate window", () => {
  const opened = [];
  const replaced = [];
  let onMessage;
  class MockWebSocket {
    constructor(url) { this.url = url; }
    set onmessage(handler) { onMessage = handler; }
    close() {}
  }
  globalThis.WebSocket = MockWebSocket;
  globalThis.location = { protocol: "http:", host: "127.0.0.1:4792" };

  let capturedEffect;
  const sessionMocks = {
    react: {
      ...React,
      useState: (init) => [init, () => {}],
      useRef: (init) => ({ current: init ?? null }),
      useEffect: (fn) => { capturedEffect = fn; },
    },
    "@tanstack/react-query": { useQuery: () => ({ data: null }) },
    "../api": { AlphaXivNotConnectedError: NotConnected, OpenResearchAccountError: AccountError },
    "@base-ui/react/menu": { Menu: {} },
    "../dialog": {},
    "../queries/settings": { getRuntimeQuery: () => ({}) },
    "../queries/invalidation": {},
    "../queries/client": { queryClient: { getQueryData: () => null } },
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
    "./confirmOpenResearchSignOut": {},
    "./OpenResearchAccount": {},
    "./ui": { showAlert: () => {} },
  };
  const mockWindow = {
    ipc: { postMessage() {} },
    open: (url, target) => { opened.push({ url, target }); return {}; },
    setTimeout: (fn) => { fn(); return 1; },
    clearTimeout: () => {},
  };
  const sessionExports = {};
  new Function("require", "exports", "window", code)(
    (name) => sessionMocks[name] ?? require(name),
    sessionExports,
    mockWindow,
  );

  const placeholderWindow = {
    location: { replace: (url) => replaced.push(url) },
    closed: false,
    close() {},
  };

  sessionExports.LoginSession({ connectOnly: true, remote: true, placeholderWindow, onComplete() {}, onError() {} });
  assert.ok(capturedEffect);
  capturedEffect();

  assert.ok(onMessage);
  const authUrl = "https://api.alphaxiv.org/auth/oauth2/authorize?client_id=123&state=abc";
  const output = `Opening your browser to sign in with alphaXiv…\nIf it doesn't open, visit:\n  ${authUrl}\n`;
  onMessage({ data: new TextEncoder().encode(output).buffer });

  assert.deepEqual(replaced, [authUrl]);
  assert.equal(opened.length, 0);
});

test("LoginSession prompts browser window on remote sessions without placeholder", () => {
  const opened = [];
  let onMessage;
  class MockWebSocket {
    constructor(url) { this.url = url; }
    set onmessage(handler) { onMessage = handler; }
    close() {}
  }
  globalThis.WebSocket = MockWebSocket;
  globalThis.location = { protocol: "http:", host: "127.0.0.1:4792" };

  let capturedEffect;
  const sessionMocks = {
    react: {
      ...React,
      useState: (init) => [init, () => {}],
      useRef: (init) => ({ current: init ?? null }),
      useEffect: (fn) => { capturedEffect = fn; },
    },
    "@tanstack/react-query": { useQuery: () => ({ data: null }) },
    "../api": { AlphaXivNotConnectedError: NotConnected, OpenResearchAccountError: AccountError },
    "@base-ui/react/menu": { Menu: {} },
    "../dialog": {},
    "../queries/settings": { getRuntimeQuery: () => ({}) },
    "../queries/invalidation": {},
    "../queries/client": { queryClient: { getQueryData: () => null } },
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
    "./confirmOpenResearchSignOut": {},
    "./OpenResearchAccount": {},
    "./ui": { showAlert: () => {} },
  };
  const mockWindow = {
    ipc: { postMessage() {} },
    open: (url, target) => { opened.push({ url, target }); return {}; },
    setTimeout: (fn) => { fn(); return 1; },
    clearTimeout: () => {},
  };
  const sessionExports = {};
  new Function("require", "exports", "window", code)(
    (name) => sessionMocks[name] ?? require(name),
    sessionExports,
    mockWindow,
  );

  sessionExports.LoginSession({ connectOnly: true, remote: true, onComplete() {}, onError() {} });
  assert.ok(capturedEffect);
  capturedEffect();

  assert.ok(onMessage);
  const authUrl = "https://api.alphaxiv.org/auth/oauth2/authorize?client_id=123&state=abc";
  const output = `Opening your browser to sign in with alphaXiv…\nIf it doesn't open, visit:\n  ${authUrl}\n`;
  onMessage({ data: new TextEncoder().encode(output).buffer });

  assert.equal(opened.length, 1);
  assert.equal(opened[0].url, authUrl);
  assert.equal(opened[0].target, "_blank");

  // A subsequent message does not prompt a second browser page
  onMessage({ data: new TextEncoder().encode(output).buffer });
  assert.equal(opened.length, 1);
});

test("LoginSession does not open browser on local sessions", () => {
  const opened = [];
  let onMessage;
  class MockWebSocket {
    constructor(url) { this.url = url; }
    set onmessage(handler) { onMessage = handler; }
    close() {}
  }
  globalThis.WebSocket = MockWebSocket;
  globalThis.location = { protocol: "http:", host: "127.0.0.1:4792" };

  let capturedEffect;
  const sessionMocks = {
    react: {
      ...React,
      useState: (init) => [init, () => {}],
      useRef: (init) => ({ current: init ?? null }),
      useEffect: (fn) => { capturedEffect = fn; },
    },
    "@tanstack/react-query": { useQuery: () => ({ data: null }) },
    "../api": { AlphaXivNotConnectedError: NotConnected, OpenResearchAccountError: AccountError },
    "@base-ui/react/menu": { Menu: {} },
    "../dialog": {},
    "../queries/settings": { getRuntimeQuery: () => ({}) },
    "../queries/invalidation": {},
    "../queries/client": { queryClient: { getQueryData: () => null } },
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
    "./confirmOpenResearchSignOut": {},
    "./OpenResearchAccount": {},
    "./ui": { showAlert: () => {} },
  };
  const mockWindow = {
    ipc: { postMessage() {} },
    open: (url, target) => { opened.push({ url, target }); return {}; },
    setTimeout: (fn) => { fn(); return 1; },
    clearTimeout: () => {},
  };
  const sessionExports = {};
  new Function("require", "exports", "window", code)(
    (name) => sessionMocks[name] ?? require(name),
    sessionExports,
    mockWindow,
  );

  sessionExports.LoginSession({ connectOnly: true, remote: false, onComplete() {}, onError() {} });
  assert.ok(capturedEffect);
  capturedEffect();

  assert.ok(onMessage);
  const authUrl = "https://api.alphaxiv.org/auth/oauth2/authorize?client_id=123&state=abc";
  const output = `Opening your browser to sign in with alphaXiv…\nIf it doesn't open, visit:\n  ${authUrl}\n`;
  onMessage({ data: new TextEncoder().encode(output).buffer });

  assert.equal(opened.length, 0);
});

test("openSignIn opens a placeholder tab in browser when remote and closes it on cancel", () => {
  const opened = [];
  const closed = [];
  const mockPlaceholder = {
    close: () => { closed.push("closed"); },
    location: { replace() {} },
  };
  const mockWindow = {
    open: (url, target) => {
      opened.push({ url, target });
      return mockPlaceholder;
    },
  };
  const dialogs = [];
  const mocks = {
    react: { ...React, useEffect() {}, useRef: () => ({ current: null }), useState: (initial) => [initial, () => {}] },
    "@tanstack/react-query": { useQuery: () => ({ data: null }) },
    "../api": { AlphaXivNotConnectedError: NotConnected, OpenResearchAccountError: AccountError },
    "@base-ui/react/menu": { Menu: {} },
    "../dialog": { showCustomDialog: (options) => { dialogs.push(options); } },
    "../queries/settings": {
      getRuntimeQuery: () => ({ queryKey: ["gateway", "runtime"] }),
      useOpenResearchAccount: () => ({ data: null }),
      useAlphaXivFolders: () => ({ error: null }),
      useSignOutOpenResearch: () => ({ isPending: false }),
    },
    "../queries/invalidation": { invalidateFamilies: () => {} },
    "../queries/client": { queryClient: { invalidateQueries: async () => {}, getQueryData: () => ({ kind: "ssh" }) }, workspaceKey: (key) => [key] },
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, name) => () => String(name) }) },
    "./confirmOpenResearchSignOut": {},
    "./OpenResearchAccount": { accountErrorMessage: () => null },
    "./ui": { Button: "button", DialogContent: "dialog-content", DialogTitle: "dialog-title", MenuItem: "button", Spinner: "spinner", showAlert: () => {} },
  };
  const exports = {};
  new Function("require", "exports", "window", code)(
    (name) => mocks[name] ?? require(name),
    exports,
    mockWindow,
  );

  exports.openSignIn(true, null, undefined, true);
  assert.equal(opened.length, 1);
  assert.equal(opened[0].url, "/remote-launch");
  assert.equal(opened[0].target, "_blank");

  assert.equal(dialogs.length, 1);
  dialogs[0].onCancel();
  assert.equal(closed.length, 1);
});

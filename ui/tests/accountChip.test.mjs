import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { act, createElement, Fragment } from "react";
import { AccountChip, LoginSession, useSignIn } from "../src/components/AccountChip.tsx";
import { Toaster } from "../src/components/ui/Toast.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, render, serve, settle } from "./render.mjs";

const user = { id: "u", firstName: "Clover", lastName: null, email: "c@example.org", alphaxivUrl: "https://www.alphaxiv.org" };
const signedIn = (overrides = {}) => ({ loggedIn: true, apiUrl: "https://api.example.org", user: { ...user, ...overrides } });
const signedOut = { loggedIn: false, apiUrl: null, user: null };
const outage = { failure: { kind: "offline", apiUrl: "https://api.example.org", status: null } };

/** The login sockets the page opened; a test plays the server's side on them. */
const sockets = [];
globalThis.WebSocket = class extends EventTarget {
  constructor(url) {
    super();
    this.url = String(url);
    sockets.push(this);
  }
  close() { this.closed = true; }
};
const output = (socket, text) => act(async () => socket.onmessage({ data: new TextEncoder().encode(text).buffer }));
const control = (socket, value) => act(async () => socket.onmessage({ data: JSON.stringify(value) }));
const authUrl = "https://api.alphaxiv.org/auth/oauth2/authorize?client_id=123&state=abc";
const prompt = `Opening your browser to sign in with alphaXiv…\nIf it doesn't open, visit:\n  ${authUrl}\n`;

afterEach(() => {
  sockets.length = 0;
  delete window.ipc;
});

/** A control elsewhere on the page that starts sign-in. */
function SignInButton() {
  const signIn = useSignIn();
  return createElement("button", { onClick: (event) => signIn(false, event.currentTarget) }, "Start sign-in");
}

/** Renders the account chip against a backend reporting `account`, and alphaXiv folders unless `alphaxiv` is false. */
async function chip(account, { alphaxiv = true, runtime = { kind: "local", version: "test", buildChannel: "stable" } } = {}) {
  const requests = [];
  serve((path, init) => {
    requests.push(`${init.method ?? "GET"} ${path}`);
    if (path === "/api/account/openresearch") return typeof account === "function" ? account() : account;
    if (path === "/api/alphaxiv/folders") return alphaxiv ? [] : new Response("", { status: 401 });
    if (path === "/api/settings/openresearch/logout") return { ok: true };
    if (path === "/_orx/runtime") return runtime;
  });
  const { container, workspace } = await render(createElement(Fragment, null, createElement(Toaster), createElement(AccountChip, { onDetails() {} }), createElement(SignInButton)));
  await settle();
  return { container, requests, workspace, trigger: () => container.querySelector("button") };
}

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};
const menuItems = () => [...document.querySelectorAll('[role="menuitem"]')].map((item) => item.textContent);
const dialog = () => document.querySelector('[role="dialog"]');
const waitForSocket = async () => {
  await act(async () => new Promise((resolve) => setTimeout(resolve, 5)));
  return sockets.at(-1);
};

for (const linked of [true, false]) {
  test(`account menu shows identity, account details and sign out; captions name ${linked ? "alphaXiv" : "OpenResearch"} as the provider`, async () => {
    const { container, requests, trigger } = await chip(signedIn({ alphaxivUrl: linked ? user.alphaxivUrl : null }));
    assert.match(trigger().textContent, /Clover/);
    await click(trigger());
    assert.match(document.body.textContent, new RegExp(linked ? "alphaXiv" : "OpenResearch"));
    assert.match(document.body.textContent, new RegExp(user.email));
    assert.deepEqual(menuItems(), [m.account_details(), m.settings_openresearch_sign_out()]);

    await click(find('[role="menuitem"]', m.settings_openresearch_sign_out()));
    assert.ok(!requests.includes("POST /api/settings/openresearch/logout"));
    await click(find('[role="dialog"] button', m.settings_openresearch_sign_out()));
    assert.ok(requests.includes("POST /api/settings/openresearch/logout"));
    assert.equal(container.querySelector("button").textContent, m.account_sign_in_alphaxiv());
  });
}

test("signed out opens the sign-in dialog; cancel rechecks the account", async () => {
  const { requests, trigger } = await chip(signedOut);
  await click(trigger());
  assert.match(dialog().textContent, new RegExp(m.account_sign_in_alphaxiv()));
  const socket = await waitForSocket();
  assert.equal(new URL(socket.url).pathname, "/api/settings/openresearch/login");

  const reads = requests.filter((request) => request === "GET /api/account/openresearch").length;
  await click(find('[role="dialog"] button', m.settings_page_cancel()));
  assert.equal(dialog(), null);
  assert.equal(requests.filter((request) => request === "GET /api/account/openresearch").length, reads + 1);
});

test("unavailable account retries instead of presenting signed out", async () => {
  let reads = 0;
  const { trigger } = await chip(() => { reads++; return outage; });
  assert.notEqual(trigger().textContent, m.account_sign_in_alphaxiv());
  await click(trigger());
  await click(find('[role="menuitem"]', m.settings_check_again()));
  assert.equal(reads, 2);
});

test("account menu starts the alphaXiv-only connection when absent", async () => {
  const { trigger } = await chip(signedIn(), { alphaxiv: false });
  await click(trigger());
  await click(find('[role="menuitem"]', m.library_connect()));
  assert.match(dialog().textContent, new RegExp(m.library_connect()));
  assert.equal(new URL((await waitForSocket()).url).pathname, "/api/settings/alphaxiv/login");
});

test("finished sign-in closes the dialog and brings the desktop window forward", async () => {
  const posted = [];
  window.ipc = { postMessage: (message) => posted.push(message) };
  let account = signedOut;
  const { requests, trigger } = await chip(() => account);
  await click(trigger());
  const socket = await waitForSocket();
  account = signedIn();
  await control(socket, { type: "complete" });
  await settle();
  assert.equal(dialog(), null);
  assert.deepEqual(posted, ["window:focus"]);
  assert.ok(requests.filter((request) => request === "GET /api/account/openresearch").length >= 2);
  assert.match(trigger().textContent, /Clover/);
});

test("failed connection offers a fresh retry", async () => {
  const { trigger } = await chip(signedIn(), { alphaxiv: false });
  await click(trigger());
  await click(find('[role="menuitem"]', m.library_connect()));
  const first = await waitForSocket();
  await control(first, { type: "error", exitCode: 1 });
  assert.equal(dialog().querySelector('[role="alert"]').textContent, m.account_alphaxiv_not_connected());

  await click(find('[role="dialog"] button', m.app_retry()));
  const second = await waitForSocket();
  assert.notEqual(second, first);
  assert.equal(dialog().querySelector('[role="alert"]'), null);
});

for (const connectOnly of [true, false]) {
  test(`OpenResearch failing to reach alphaXiv asks to try again in a moment${connectOnly ? " when connecting" : ""}`, async () => {
    const { trigger } = await chip(connectOnly ? signedIn() : signedOut, { alphaxiv: !connectOnly });
    await click(trigger());
    if (connectOnly) await click(find('[role="menuitem"]', m.library_connect()));
    await control(await waitForSocket(), { type: "error", exitCode: 75 });
    assert.equal(dialog().querySelector('[role="alert"]').textContent, m.account_alphaxiv_unavailable());
  });
}

test("an account outage hides the alphaXiv connection action even with cached identity", async () => {
  let reads = 0;
  const { trigger, workspace } = await chip(() => ++reads === 1 ? signedIn() : outage, { alphaxiv: false });
  await act(async () => workspace.queryClient.refetchQueries());
  await settle();
  await click(trigger());
  assert.ok(menuItems().includes(m.settings_check_again()));
  assert.ok(!menuItems().includes(m.library_connect()));
});

test("sign-in started elsewhere returns focus to the control that started it", async () => {
  await chip(signedOut);
  const opener = find("button", "Start sign-in");
  await click(opener);
  await click(find('[role="dialog"] button', m.settings_page_cancel()));
  assert.equal(document.activeElement === opener, true);
});

for (const [name, remote, placeholder, expected] of [
  ["navigates the placeholder tab on remote sessions without opening a duplicate window", true, true, { replaced: [authUrl], opened: [] }],
  ["opens one browser window on remote sessions without a placeholder", true, false, { replaced: [], opened: [authUrl] }],
  ["does not open a browser on local sessions", false, false, { replaced: [], opened: [] }],
]) {
  test(`LoginSession ${name}`, async (t) => {
    const replaced = [];
    const opened = t.mock.method(window, "open", () => ({}));
    const placeholderWindow = placeholder ? { location: { replace: (url) => replaced.push(url) }, closed: false, close() {} } : null;
    await render(createElement(LoginSession, { connectOnly: true, remote, placeholderWindow, onComplete() {}, onError() {} }));
    const socket = await waitForSocket();
    await output(socket, prompt);
    await output(socket, prompt);
    assert.deepEqual({ replaced, opened: opened.mock.calls.map(({ arguments: [url] }) => url) }, expected);
    assert.ok(opened.mock.calls.every(({ arguments: [, target] }) => target === "_blank"));
  });
}

test("sign-in opens a placeholder tab in browser when remote and closes it on cancel", async (t) => {
  const placeholder = { closed: false, close() { this.closed = true; }, location: { replace() {} } };
  const opened = t.mock.method(window, "open", () => placeholder);
  await chip(signedOut, { runtime: { kind: "ssh", version: "test", buildChannel: "stable", session: { id: "r", host: "gpu", status: "connected" } } });
  await click(find("button", "Start sign-in"));
  assert.deepEqual(opened.mock.calls.map(({ arguments: args }) => args), [["/remote-launch", "_blank"]]);
  await click(find('[role="dialog"] button', m.settings_page_cancel()));
  assert.equal(placeholder.closed, true);
});

import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { act, createElement, Fragment } from "react";
import { AccountChip, LoginSession, useSignIn } from "../src/components/AccountChip.tsx";
import { Toaster } from "../src/components/ui/Toast.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, render, renderApp, serve, serveApp, settle } from "./render.mjs";

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
    if (path === "/api/account/openresearch") return requests.includes("POST /api/settings/openresearch/logout") ? signedOut : typeof account === "function" ? account() : account;
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
    assert.equal(container.querySelector("button").textContent, m.settings_sign_in());
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
  assert.notEqual(trigger().textContent, m.settings_sign_in());
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

test("OpenResearch failing to reach alphaXiv asks to try again in a moment", async () => {
  const { trigger } = await chip(signedOut);
  await click(trigger());
  await control(await waitForSocket(), { type: "error", exitCode: 75 });
  assert.equal(dialog().querySelector('[role="alert"]').textContent, m.account_alphaxiv_unavailable());
});

test("a failed alphaXiv connection shows the server's reason and offers a retry", async () => {
  const { trigger } = await chip(signedIn(), { alphaxiv: false });
  await click(trigger());
  await click(find('[role="menuitem"]', m.library_connect()));
  const reason = "The sign-in was denied on alphaXiv. Try again to sign in.";
  await control(await waitForSocket(), { type: "error", error: reason });
  assert.equal(dialog().querySelector('[role="alert"]').textContent, reason);
  assert.ok(find('[role="dialog"] button', m.app_retry()));
});

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

/** A tab the sign-in reserved during the click, recording where it was sent. */
function reservedTab() {
  const tab = { closed: false, sent: [], close() { tab.closed = true; } };
  tab.location = { replace: (url) => tab.sent.push(url) };
  return tab;
}
const deviceUrl = "https://www.alphaxiv.org/device?user_code=ABCD-EFGH";
const session = async (props) => {
  await render(createElement(LoginSession, { connectOnly: true, tab: null, onComplete() {}, onError() {}, ...props }));
  return waitForSocket();
};
const status = () => document.querySelector('[role="status"]').textContent;

test("a code to approve sends the reserved tab to alphaXiv once and shows the code", async (t) => {
  const opened = t.mock.method(window, "open", () => ({}));
  const tab = reservedTab();
  const socket = await session({ tab });
  assert.equal(new URL(socket.url).pathname, "/api/settings/alphaxiv/login");
  await control(socket, { type: "authUrl", url: deviceUrl, code: "ABCD-EFGH" });
  await control(socket, { type: "authUrl", url: deviceUrl, code: "ABCD-EFGH" });
  assert.deepEqual(tab.sent, [deviceUrl]);
  assert.equal(opened.mock.callCount(), 0);
  assert.equal(status(), m.account_waiting_approval());
  assert.match(document.body.textContent, /ABCD-EFGH/);
  assert.equal(find("a", m.account_open_alphaxiv()).getAttribute("href"), deviceUrl);
});

test("without a reserved tab, a code to approve opens alphaXiv in one window", async (t) => {
  const opened = t.mock.method(window, "open", () => ({}));
  const socket = await session({});
  await control(socket, { type: "authUrl", url: deviceUrl, code: "ABCD-EFGH" });
  await control(socket, { type: "authUrl", url: deviceUrl, code: "ABCD-EFGH" });
  assert.deepEqual(opened.mock.calls.map(({ arguments: args }) => args), [[deviceUrl, "_blank"]]);
});

test("when the server opens its own browser, a reserved tab closes and the link stays", async (t) => {
  const opened = t.mock.method(window, "open", () => ({}));
  const tab = reservedTab();
  const socket = await session({ tab });
  await control(socket, { type: "authUrl", url: authUrl, code: null });
  assert.equal(tab.closed, true);
  assert.deepEqual(tab.sent, []);
  assert.equal(opened.mock.callCount(), 0);
  assert.equal(status(), m.account_finish_browser());
  assert.equal(find("a", authUrl).getAttribute("href"), authUrl);
});

test("a prompt that isn't a web page is never opened", async (t) => {
  const opened = t.mock.method(window, "open", () => ({}));
  const tab = reservedTab();
  const socket = await session({ tab });
  await control(socket, { type: "authUrl", url: "javascript:alert(1)", code: "ABCD-EFGH" });
  assert.deepEqual(tab.sent, []);
  assert.equal(opened.mock.callCount(), 0);
  assert.doesNotMatch(document.body.textContent, /ABCD-EFGH/);
});

test("OpenResearch sign-in in this computer's browser only links the address in its output", async (t) => {
  const opened = t.mock.method(window, "open", () => ({}));
  const socket = await session({ connectOnly: false, tab: null });
  assert.equal(new URL(socket.url).pathname, "/api/settings/openresearch/login");
  await output(socket, prompt);
  await output(socket, prompt);
  assert.equal(opened.mock.callCount(), 0);
  assert.equal(find("a", authUrl).getAttribute("href"), authUrl);
});

test("OpenResearch sign-in answers the cursor query ConPTY waits on before printing", async () => {
  const socket = await session({ connectOnly: false, tab: null });
  const replies = [];
  socket.readyState = WebSocket.OPEN;
  socket.send = (data) => replies.push(new TextDecoder().decode(data));
  await output(socket, "\x1b[6n");
  await settle();
  assert.match(replies.join(""), /^\x1b\[\d+;\d+R$/);
});

test("OpenResearch sign-in widens the terminal so Windows does not wrap the URL", async (t) => {
  const opened = t.mock.method(window, "open", () => ({}));
  const socket = await session({ connectOnly: false, tab: null });
  const sent = [];
  socket.send = (data) => sent.push(data);
  await act(async () => socket.onopen());
  assert.deepEqual(JSON.parse(sent[0]), { type: "resize", cols: 1000, rows: 24 });
  await output(socket, `\x1b[?25hOpening your browser to sign in with alphaXiv…\r\n\x1b[25lIf it doesn't open, visit:\x1b[348X\r\n  ${authUrl}\x1b[5;1H\x1b[?25h`);
  assert.equal(opened.mock.callCount(), 0);
  assert.equal(find("a", authUrl).getAttribute("href"), authUrl);
});

test("OpenResearch sign-in with a code sends the reserved tab to alphaXiv once and shows the code", async (t) => {
  const opened = t.mock.method(window, "open", () => ({}));
  const tab = reservedTab();
  const socket = await session({ connectOnly: false, tab });
  const approval = "https://www.alphaxiv.org/device?user_code=FSBNGVDG";
  await output(socket, "To sign in with alphaXiv, open this link on any device and approve the code FSBNGVDG:\r\n");
  await output(socket, `  ${approval}\r\n\r\n`);
  await output(socket, "\r\n");
  assert.deepEqual(tab.sent, [approval]);
  assert.equal(opened.mock.callCount(), 0);
  assert.match(document.body.textContent, /FSBNGVDG/);
  assert.equal(find("a", m.account_open_alphaxiv()).getAttribute("href"), approval);
});

test("a browser on another machine reserves a tab during the click and closes it on cancel", async (t) => {
  window.happyDOM.setURL("http://192.168.1.5/");
  t.after(() => window.happyDOM.setURL("http://localhost/"));
  const tab = reservedTab();
  const opened = t.mock.method(window, "open", () => tab);
  await chip(signedOut);
  await click(find("button", "Start sign-in"));
  assert.deepEqual(opened.mock.calls.map(({ arguments: args }) => args), [["/remote-launch", "_blank"]]);
  await click(find('[role="dialog"] button', m.settings_page_cancel()));
  assert.equal(tab.closed, true);
});

test("Escape after a retry closes the tab the retry reserved", async (t) => {
  window.happyDOM.setURL("http://192.168.1.5/");
  t.after(() => window.happyDOM.setURL("http://localhost/"));
  const tabs = [];
  t.mock.method(window, "open", () => { tabs.push(reservedTab()); return tabs.at(-1); });
  const { trigger } = await chip(signedIn(), { alphaxiv: false });
  await click(trigger());
  await click(find('[role="menuitem"]', m.library_connect()));
  await control(await waitForSocket(), { type: "error", error: "Denied." });
  await click(find('[role="dialog"] button', m.app_retry()));
  await act(async () => document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  await settle();
  assert.equal(dialog(), null);
  assert.deepEqual(tabs.map((tab) => tab.closed), [true, true]);
});

test("a laptop window on a remote workspace reserves no tab, since the laptop signs in", async (t) => {
  const opened = t.mock.method(window, "open", () => reservedTab());
  await chip(signedOut, { runtime: { kind: "ssh", version: "test", buildChannel: "stable", session: { id: "r", host: "gpu", status: "connected" } } });
  await click(find("button", "Start sign-in"));
  assert.equal(opened.mock.callCount(), 0);
});

test("the reserved tab says where it is going", async () => {
  serveApp();
  await renderApp("/remote-launch");
  assert.equal(status(), m.account_continuing_to_alphaxiv());
});

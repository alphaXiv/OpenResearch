import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { Onboarding } from "../src/components/Onboarding.tsx";
import { ltr } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { find, navigate, render, renderApp, serve, serveApp, settle } from "./render.mjs";

/** The page's sockets; a test plays the server's side on them. */
const sockets = [];
globalThis.WebSocket = class extends EventTarget {
  static OPEN = 1;
  readyState = 1;
  constructor(url) {
    super();
    this.url = String(url);
    sockets.push(this);
  }
  send() {}
  close() { this.readyState = 3; }
};

const runtime = {
  kind: "ssh", version: "test", buildChannel: "stable", dashboardProtocol: 1,
  session: { id: "gpu", host: "gpu-lab", user: "me", status: "connected", version: "test", dashboardProtocol: 1, gatewayUrl: "http://localhost:4910/", error: null, repairCommand: null, installPaths: null, uiPreferences: { theme: null, locale: null }, canStartNewHost: true },
};
const codex = { id: "codex", name: "Codex", installed: true, agentReady: true, models: [], options: {} };
const claude = () => ({ id: "claude-code", name: "Claude Code", installed: false, agentReady: false, models: [], options: {} });
const datacenter = () => ({ ip: "192.0.2.1", isDatacenter: true });

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};
const button = (label) => find("button", label);
const warnings = () => document.querySelectorAll('[role="alertdialog"]').length;
const setupDialogs = () => [...document.querySelectorAll('[role="dialog"]')].filter((dialog) => dialog.textContent.includes("Claude Code")).length;

/** A fresh remote host's first page, whose agents are `harnesses` and whose public network lookup answers `network`. */
async function remoteSetup({ harnesses = [codex, claude()], network = datacenter } = {}) {
  const requests = [];
  serveApp({
    "GET /_orx/runtime": () => runtime,
    "GET /api/settings/ui-state": () => ({ onboardingCompleted: false, preferredAgent: null }),
    "GET /api/harnesses": () => ({ harnesses }),
    "GET /api/harnesses/setup/commands": () => ({ "claude-code": { install: "install", login: "login" } }),
    "GET /api/settings/network": () => network(),
  });
  const backend = globalThis.fetch;
  globalThis.fetch = (input, init = {}) => {
    const url = new URL(String(input), location.href);
    requests.push({ request: `${init.method ?? "GET"} ${url.pathname}`, body: init.body && JSON.parse(init.body) });
    return backend(input, init);
  };
  await renderApp("/projects");
  return { harnesses, requests: (request) => requests.filter((entry) => entry.request === request) };
}

test("remote setup starts at agents and saves only the machine's preferred agent", async () => {
  const setup = await remoteSetup();
  const text = document.body.textContent;
  assert.ok(text.includes(m.remote_agents_title({ host: ltr("gpu-lab") })));
  assert.ok(text.includes(m.remote_agents_description({ host: ltr("gpu-lab") })));
  for (const local of [m.onboarding_step_1_of_2(), m.onboarding_back(), m.onboarding_alphaxiv_title()]) assert.ok(!text.includes(local), local);
  assert.deepEqual(setup.requests("GET /api/project-path/status"), []);

  await click(button("Codex"));
  await click(button(m.onboarding_continue()));
  const writes = setup.requests("POST /api/settings/ui-state").map(({ body }) => body).filter((body) => body.preferredAgent);
  assert.equal(writes.length, 1);
  assert.deepEqual(Object.keys(writes[0]), ["preferredAgent"]);
  assert.equal(writes[0].preferredAgent.harness, "codex");
  assert.equal(setup.requests("POST /api/onboarding/complete").length, 0);
  assert.ok(!document.body.textContent.includes(m.remote_agents_title({ host: ltr("gpu-lab") })));
});

test("a datacenter warning precedes Claude setup once across install and sign in", async () => {
  const setup = await remoteSetup();
  await click(button(m.harness_setup_install()));
  assert.equal(warnings(), 1);
  assert.ok(document.querySelector('[role="alertdialog"]').textContent.includes(m.remote_claude_datacenter_description({ host: ltr("gpu-lab"), ip: ltr("192.0.2.1") })));
  await click(button(m.remote_claude_datacenter_continue()));
  assert.equal(warnings(), 0);
  assert.equal(setupDialogs(), 1);

  Object.assign(setup.harnesses[1], { installed: true, authState: "needsLogin", loginEligible: true });
  await click(button(m.onboarding_re_check()));
  await click(button(m.harness_setup_login()));
  assert.equal(warnings(), 0);
  assert.equal(setupDialogs(), 2);
});

test("dismissing the warning does not repeat it on the next attempt", async () => {
  await remoteSetup();
  await click(button(m.harness_setup_install()));
  await click(find('[role="alertdialog"] button', m.common_cancel()));
  assert.equal(setupDialogs(), 0);
  await click(button(m.harness_setup_install()));
  assert.equal(warnings(), 0);
  assert.equal(setupDialogs(), 1);
});

for (const [verdict, network] of [["no datacenter", () => ({ ip: "192.0.2.1", isDatacenter: false })], ["an unknown network", () => ({ ip: "192.0.2.1", isDatacenter: null })], ["a failed lookup", () => Response.json({ error: "lookup failed" }, { status: 502 })]]) {
  test(`Claude setup opens directly with ${verdict}`, async () => {
    await remoteSetup({ network });
    await click(button(m.harness_setup_install()));
    assert.equal(warnings(), 0);
    assert.equal(setupDialogs(), 1);
  });
}

test("leaving agent setup cancels a delayed Claude install prompt", async () => {
  let release;
  await remoteSetup({ network: () => new Promise((resolve) => { release = () => resolve(Response.json(datacenter())); }) });
  await click(button(m.harness_setup_install()));
  assert.equal(warnings() + setupDialogs(), 0);
  await navigate("/remote-launch");
  await act(async () => release());
  await settle();
  assert.equal(warnings() + setupDialogs(), 0);
});

test("Continue explains when the chosen agent is still loading", async () => {
  const setup = await remoteSetup({ harnesses: [{ ...codex, catalogPending: true }, { ...codex, id: "opencode", name: "OpenCode" }] });
  await click(button("Codex"));
  await click(button(m.onboarding_continue()));
  assert.equal(document.querySelector('[role="alert"]')?.textContent, m.harness_setup_not_ready());
  assert.equal(setup.requests("POST /api/settings/ui-state").filter(({ body }) => body.preferredAgent).length, 0);
});

const signedIn = { loggedIn: true, apiUrl: "https://api.example.org", user: { id: "u", firstName: "Ada", lastName: null, email: "ada@example.org", alphaxivUrl: "https://www.alphaxiv.org" } };
const signedOut = { loggedIn: false, apiUrl: null, user: null };
const notConnected = () => new Response("", { status: 401 });

/** The local walkthrough's last step, against a backend whose account is `account` and whose alphaXiv profile answers `me`. */
async function alphaXivStep({ account, me = () => ({ avatarUrl: null }) }) {
  const completions = [];
  const done = [];
  serve((path, init) => {
    if (path === "/_orx/runtime") return { kind: "local", version: "test", buildChannel: "stable" };
    if (path === "/api/harnesses") return { harnesses: [codex] };
    if (path === "/api/harnesses/setup/commands") return {};
    if (path === "/api/project-path/status") return { gitVersion: "git 2.49.0" };
    if (path === "/api/telemetry/event") return { ok: true };
    if (path === "/api/account/openresearch") return account;
    if (path === "/api/alphaxiv/me") return me();
    if (path === "/api/onboarding/complete") {
      completions.push(JSON.parse(init.body));
      return { project: { id: "demo" }, selection: JSON.parse(init.body) };
    }
  });
  await render(createElement(Onboarding, { remote: false, preferredAgent: null, onDone: (project) => done.push(project) }));
  await settle();
  await click(button(m.onboarding_continue()));
  await click(button(m.onboarding_continue()));
  assert.ok(document.body.textContent.includes(m.onboarding_alphaxiv_title()));
  return { completions, done };
}

for (const [name, options, login] of [
  ["signed out", { account: signedOut }, "/api/settings/openresearch/login"],
  ["signed in to alphaXiv", { account: signedIn }, null],
  ["signed in without an alphaXiv connection", { account: signedIn, me: notConnected }, "/api/settings/alphaxiv/login"],
]) {
  test(`the alphaXiv step offers to finish by skipping only until alphaXiv is linked: ${name}`, async () => {
    await alphaXivStep(options);
    assert.equal(!!button(m.onboarding_skip()), !!login);
    assert.equal(!!button(m.onboarding_get_started()), !login);
    assert.equal(document.body.textContent.includes(m.onboarding_signed_in()), !login);
    const signIn = button(m.account_sign_in_alphaxiv());
    assert.equal(!!signIn, !!login);
    if (!signIn) return;
    await click(signIn);
    assert.equal(new URL(sockets.at(-1).url).pathname, login);
  });
}

test("finishing alphaXiv sign-in completes onboarding with the chosen agent", async () => {
  const step = await alphaXivStep({ account: signedOut });
  await click(button(m.account_sign_in_alphaxiv()));
  await act(async () => sockets.at(-1).onmessage({ data: JSON.stringify({ type: "complete" }) }));
  await settle();
  assert.equal(step.completions.length, 1);
  assert.equal(step.completions[0].harness, "codex");
  assert.deepEqual(step.done, [{ id: "demo" }]);
});

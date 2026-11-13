import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { Onboarding } from "../src/components/Onboarding.tsx";
import { ltr } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { find, render, renderApp, serve, serveApp, settle, type } from "./render.mjs";
import { model } from "./models.mjs";

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
const codex = { id: "codex", name: "Codex", installed: true, authenticated: true, authState: "ready", agentReady: true, models: [model("gpt-6.1-sol")], defaultModel: "gpt-6.1-sol", options: {} };
const claude = () => ({ id: "claude-code", name: "Claude Code", installed: false, agentReady: false, models: [], options: {} });
const opencode = (models, extra = {}) => ({
  id: "opencode", name: "OpenCode", installed: true, installBroken: false, version: "2.1.0", authenticated: true, authState: "ready",
  agentReady: true, models: models.map((id) => model(id)), defaultModel: models[0], options: {}, ...extra,
});
const datacenter = () => ({ ip: "192.0.2.1", isDatacenter: true });

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};
const button = (label) => find("button", label);
const wizardButton = (label) => find('[role="dialog"] button', label);
const warnings = () => document.querySelectorAll('[role="alertdialog"]').length;

/** A fresh remote host's first page, whose agents are `harnesses` and whose public network lookup answers `network`. */
async function remoteSetup({ harnesses = [codex, claude()], providers = [], network = datacenter, routes = {} } = {}) {
  const requests = [];
  serveApp({
    "GET /_orx/runtime": () => runtime,
    "GET /api/settings/ui-state": () => ({ onboardingCompleted: false, preferredAgent: null }),
    "GET /api/harnesses": () => ({ harnesses }),
    "GET /api/harnesses/setup/commands": () => ({ "claude-code": { install: "install", login: "login" }, opencode: { install: "install", login: "login" }, codex: { install: "install", login: "login" } }),
    "GET /api/harnesses/acp": () => [],
    "GET /api/harnesses/acp/presets": () => [],
    "GET /api/opencode/providers": () => providers,
    "GET /api/local-models": () => [],
    "GET /api/settings/network": () => network(),
    "POST /api/agent-profiles": () => ({ id: "claude-work" }),
    "DELETE /api/agent-profiles/claude-work": () => ({ ok: true }),
    ...routes,
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

/** Starts a Claude Code sign-in from the page's Add agent wizard. */
async function addClaude() {
  await click(wizardButton("Claude Code") ?? button("Claude Code"));
}

const preferredAgentWrites = (setup) => setup.requests("POST /api/settings/ui-state").map(({ body }) => body).filter((body) => body.preferredAgent);

test("remote setup lists the agents set up, offers the rest, and saves only the machine's preferred agent", async () => {
  const setup = await remoteSetup({ harnesses: [codex, claude(), opencode([], { installed: false, agentReady: false, version: undefined })] });
  const text = document.body.textContent;
  assert.ok(text.includes(m.remote_agents_title({ host: ltr("gpu-lab") })));
  assert.ok(text.includes(m.remote_agents_description({ host: ltr("gpu-lab") })));
  for (const local of [m.onboarding_step_1_of_2(), m.onboarding_back(), m.onboarding_alphaxiv_title()]) assert.ok(!text.includes(local), local);
  assert.deepEqual(setup.requests("GET /api/project-path/status"), []);
  assert.ok(button(m.harness_setup_install()), "the free models offer OpenCode's install");
  assert.ok(button("Claude Code"), "an agent not set up yet is one click from its sign-in");
  assert.ok(document.querySelector(`button[aria-label="${m.settings_agent_use({ name: "Codex" })}"]`), "an agent set up is listed");
  assert.ok(text.includes(m.onboarding_sign_in_more_accounts()), "with an agent listed, the tiles offer another account");

  await click(button(m.onboarding_continue()));
  const writes = preferredAgentWrites(setup);
  assert.equal(writes.length, 1);
  assert.deepEqual(Object.keys(writes[0]), ["preferredAgent"]);
  assert.equal(writes[0].preferredAgent.harness, "codex");
  assert.equal(writes[0].preferredAgent.model, "gpt-6.1-sol");
  assert.equal(setup.requests("POST /api/onboarding/complete").length, 0);
  assert.ok(!document.body.textContent.includes(m.remote_agents_title({ host: ltr("gpu-lab") })));
});

test("with no agent signed in, Continue waits and says why", async () => {
  await remoteSetup({ harnesses: [claude(), { ...codex, agentReady: false, authState: "needsLogin", defaultModel: undefined, models: [] }] });
  const proceed = button(m.onboarding_continue());
  assert.ok(proceed.disabled);
  assert.equal(document.getElementById(proceed.getAttribute("aria-describedby"))?.textContent, m.onboarding_sign_in_agent_to_continue());
  assert.ok(document.body.textContent.includes(m.onboarding_sign_in_ai_provider()));
  assert.ok(button("OpenAI Codex"), "Codex is one click from its sign-in");
});

test("a provider added in the wizard is what the first chat starts on", async () => {
  const providers = [];
  const setup = await remoteSetup({
    harnesses: [codex, opencode(["opencode/big-pickle"])],
    providers,
    routes: {
      "POST /api/opencode/providers": () => {
        providers.push({ id: "deepseek", kind: "key", name: "DeepSeek" });
        setup.harnesses[1] = opencode(["opencode/big-pickle", "deepseek/deepseek-v4"]);
        return { id: "deepseek" };
      },
    },
  });
  await click(button(m.add_agent_other_providers()));
  assert.ok(!find('[role="dialog"]', m.add_agent_recommended()), "the page already offers the recommended agents");
  await click(wizardButton("DeepSeek"));
  await type(find('[role="dialog"] input[type="password"]'), "sk-test");
  await click(wizardButton(m.onboarding_continue()));
  await click(wizardButton(m.add_agent_finish({ name: "DeepSeek" })));
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 0);

  await click(button(m.onboarding_continue()));
  const [write] = preferredAgentWrites(setup);
  assert.equal(write.preferredAgent.harness, "opencode");
  assert.equal(write.preferredAgent.model, "deepseek/deepseek-v4", "a provider's own model, not OpenCode's default");
});

test("an account added here can be removed from its row after a confirm", async () => {
  const setup = await remoteSetup({ harnesses: [codex, { ...codex, profile: { id: "claude-work", name: "Codex 2" } }] });
  await click(document.querySelector(`button[aria-label="${m.settings_remove_agent({ name: ltr("Codex 2") })}"]`));
  assert.equal(setup.requests("DELETE /api/agent-profiles/claude-work").length, 0, "nothing goes before the confirm");
  await click(find('[role="dialog"] button', m.chat_panel_remove()));
  assert.equal(setup.requests("DELETE /api/agent-profiles/claude-work").length, 1);
});

test("a second Codex account is named after the agent, not its tile", async () => {
  const setup = await remoteSetup();
  await click(button("OpenAI Codex"));
  assert.equal(setup.requests("POST /api/agent-profiles")[0].body.name, "Codex 2");
});

test("removing the provider added in the wizard starts the first chat elsewhere", async () => {
  const providers = [];
  const setup = await remoteSetup({
    harnesses: [codex, opencode(["opencode/big-pickle"])],
    providers,
    routes: {
      "POST /api/opencode/providers": () => {
        providers.push({ id: "deepseek", kind: "key", name: "DeepSeek" });
        setup.harnesses[1] = opencode(["opencode/big-pickle", "deepseek/deepseek-v4"]);
        return { id: "deepseek" };
      },
      "DELETE /api/opencode/providers/deepseek": () => {
        providers.length = 0;
        setup.harnesses[1] = opencode(["opencode/big-pickle"]);
        return { ok: true };
      },
    },
  });
  await click(button(m.add_agent_other_providers()));
  await click(wizardButton("DeepSeek"));
  await type(find('[role="dialog"] input[type="password"]'), "sk-test");
  await click(wizardButton(m.onboarding_continue()));
  await click(wizardButton(m.add_agent_finish({ name: "DeepSeek" })));

  await click(document.querySelector(`button[aria-label="${m.settings_remove_agent({ name: ltr("DeepSeek") })}"]`));
  await click(find('[role="dialog"] button', m.chat_panel_remove()));
  await click(button(m.onboarding_continue()));
  const [write] = preferredAgentWrites(setup);
  assert.notEqual(write.preferredAgent.model, "deepseek/deepseek-v4", "a removed provider's model isn't the first chat's");
});

test("signing a Claude Code account in again from its row warns about a datacenter first", async () => {
  await remoteSetup({ harnesses: [codex, claude(), { ...claude(), installed: true, authState: "needsLogin", profile: { id: "claude-work", name: "Claude Code 2" } }] });
  await click(button(m.harness_setup_login()));
  assert.equal(warnings(), 1);
  await click(button(m.remote_claude_datacenter_continue()));
  assert.equal(warnings(), 0);
  assert.ok(find('[role="dialog"]', "Claude Code"), "the sign-in opens");
});

test("a datacenter warning precedes a Claude sign-in once", async () => {
  const setup = await remoteSetup();
  await addClaude();
  assert.equal(warnings(), 1);
  assert.ok(document.querySelector('[role="alertdialog"]').textContent.includes(m.remote_claude_datacenter_description({ host: ltr("gpu-lab"), ip: ltr("192.0.2.1") })));
  assert.equal(setup.requests("POST /api/agent-profiles").length, 0);
  await click(button(m.remote_claude_datacenter_continue()));
  assert.equal(warnings(), 0);
  assert.equal(setup.requests("POST /api/agent-profiles").length, 1);

  await click(wizardButton(m.onboarding_back()));
  await addClaude();
  assert.equal(warnings(), 0);
  assert.equal(setup.requests("POST /api/agent-profiles").length, 2);
});

test("dismissing the warning goes back to the page without repeating it", async () => {
  const setup = await remoteSetup();
  await addClaude();
  await click(find('[role="alertdialog"] button', m.common_cancel()));
  assert.equal(document.querySelectorAll('[role="dialog"], [role="alertdialog"]').length, 0);
  assert.equal(setup.requests("POST /api/agent-profiles").length, 0);
  await addClaude();
  assert.equal(warnings(), 0);
  assert.equal(setup.requests("POST /api/agent-profiles").length, 1);
});

for (const [verdict, network] of [["no datacenter", () => ({ ip: "192.0.2.1", isDatacenter: false })], ["an unknown network", () => ({ ip: "192.0.2.1", isDatacenter: null })], ["a failed lookup", () => Response.json({ error: "lookup failed" }, { status: 502 })]]) {
  test(`Claude sign-in starts directly with ${verdict}`, async () => {
    const setup = await remoteSetup({ network });
    await addClaude();
    assert.equal(warnings(), 0);
    assert.equal(setup.requests("POST /api/agent-profiles").length, 1);
  });
}

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

import assert from "node:assert/strict";
import test from "node:test";
import { Activity, act, createElement } from "react";
import { Onboarding } from "../src/components/Onboarding.tsx";
import { ltr } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { find, navigate, render, renderApp, serve, serveApp, settle } from "./render.mjs";

globalThis.WebSocket = class extends EventTarget {
  static OPEN = 1;
  readyState = 1;
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
  for (const local of [m.onboarding_step_1_of_2(), m.onboarding_back(), m.onboarding_tell_us_about_your_research()]) assert.ok(!text.includes(local), local);
  assert.deepEqual([...setup.requests("GET /api/settings/profile"), ...setup.requests("GET /api/project-path/status")], []);

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

test("showing a hidden machine again keeps unsaved research profile answers", async () => {
  let profileReads = 0;
  serve((path) => {
    if (path === "/api/settings/profile") { profileReads++; return { researchAreas: ["RL"], otherArea: null, background: "saved", papers: [] }; }
    if (path === "/api/harnesses") return { harnesses: [codex] };
    if (path === "/api/harnesses/setup/commands") return {};
    if (path === "/api/project-path/status") return { gitVersion: "git 2.49.0" };
    if (path === "/_orx/runtime") return { kind: "local", version: "test", buildChannel: "stable" };
  });
  const page = (mode) => createElement(Activity, { mode }, createElement(Onboarding, { remote: false, preferredAgent: null, onDone() {} }));
  const { rerender } = await render(page("visible"));
  await settle();
  assert.equal(profileReads, 1);
  await rerender(page("hidden"));
  await rerender(page("visible"));
  await settle();
  assert.equal(profileReads, 1);
});

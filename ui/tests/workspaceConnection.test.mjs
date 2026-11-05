import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { ProgressBar } from "../src/components/ProgressBar.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, press, render, renderApp, serveApp, settle } from "./render.mjs";

const session = {
  id: "s", host: "research", user: "me", status: "connected", version: "test", dashboardProtocol: 1,
  gatewayUrl: "http://localhost:4910/", error: null, repairCommand: null, installPaths: null,
  uiPreferences: { theme: null, locale: null }, canStartNewHost: true,
};
const runtimes = {
  local: { kind: "local", version: "test", buildChannel: "stable" },
  ssh: { kind: "ssh", version: "test", buildChannel: "stable", dashboardProtocol: 1, session },
};
const server = { ...session, id: "server" };
const click = async (element) => {
  await act(async () => element.click());
  await settle();
};
const visible = "[data-machine]:not([hidden])";
const dialogTitles = () => [...document.querySelectorAll(`${visible} [role="dialog"] h2`)].map((title) => title.textContent);
const visibleMachine = () => document.querySelector(visible).dataset.machine;
const switcher = () => [...document.querySelectorAll(`${visible} [data-machine-switcher]`)].at(-1);
const menuItem = (text) => find(`${visible} [role="menuitemradio"], ${visible} [role="menuitem"]`, text);

/**
 * The app over a laptop that knows the remote machine `server`, whose own server answers through the laptop's
 * gateway as the same backend; `routes` answer first. Every request lands in the returned list.
 */
async function laptop(path, routes = {}) {
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project", updatedAt: 1 }] }),
    "GET /api/projects/activity": () => ({ activity: [] }),
    "GET /api/remote/sessions": () => ({ sessions: [server] }),
    "GET /api/settings/ssh": () => ({ hosts: [{ host: "research" }], defaultHost: null }),
    "GET /api/settings/ssh/config": () => ({ path: "/tmp/config", content: "Host research" }),
    ...routes,
  });
  const backend = globalThis.fetch;
  const requests = [];
  const gateway = "/_orx/workspaces/server";
  globalThis.fetch = async (input, init = {}) => {
    const url = new URL(String(input), location.href);
    requests.push(`${init.method ?? "GET"} ${url.pathname}`);
    if (!url.pathname.startsWith(`${gateway}/`)) return backend(input, init);
    if (url.pathname === `${gateway}/_orx/runtime`) return Response.json({ ...runtimes.ssh, session: server });
    return backend(url.pathname.slice(gateway.length) + url.search, init);
  };
  await renderApp(path);
  return requests;
}

for (const [place, path] of [["home", "/projects"], ["sidebar", "/projects/p/tasks/new"]]) {
  test(`the ${place} machine menu switches between retained machines and configures the laptop from either`, async () => {
    const requests = await laptop(path);
    assert.equal(visibleMachine(), "local");
    await click(switcher());
    await click(menuItem("research"));
    assert.equal(visibleMachine(), "server");

    await click(switcher());
    await click(menuItem(m.remote_dialog_title()));
    assert.deepEqual(dialogTitles(), [m.remote_dialog_title()]);
    await click(find(`${visible} [role="dialog"] button`, m.ssh_configure_hosts()));
    assert.deepEqual(dialogTitles(), [m.ssh_config_title()]);
    assert.ok(requests.includes("GET /api/settings/ssh/config"));
    assert.ok(!requests.some((request) => request.endsWith("/workspaces/server/api/settings/ssh/config")));
    await click(find(`${visible} [role="dialog"] button`, m.settings_page_cancel()));
    assert.deepEqual(dialogTitles(), [m.remote_dialog_title()]);
    await press("Escape");
    await settle();
    assert.deepEqual(dialogTitles(), []);

    await click(switcher());
    await click(menuItem(m.projects_local()));
    assert.equal(visibleMachine(), "local");
  });
}

for (const kind of ["local", "ssh"]) {
  for (const scenario of ["loading", "error", "onboarding", "no-projects", "projects"]) {
    test(`${kind} project home always offers machine selection during ${scenario}`, async () => {
      serveApp({
        "GET /_orx/runtime": () => runtimes[kind],
        "GET /api/projects": () => {
          if (scenario === "loading") return new Promise(() => {});
          if (scenario === "error") return Response.json({ error: "offline" }, { status: 503 });
          return { projects: scenario === "projects" ? [{ id: "p", name: "Project" }] : [] };
        },
        "GET /api/settings/ui-state": () => ({ onboardingCompleted: scenario !== "onboarding" }),
        "GET /api/projects/p/ui-state": () => new Promise(() => {}),
        "GET /api/remote/sessions": () => ({ sessions: [session] }),
      });
      await renderApp("/projects");
      assert.equal(switcher().getAttribute("aria-label"), m.remote_machine_switcher({ machine: kind === "ssh" ? "research" : m.projects_local() }));
    });
  }
}

for (const outcome of ["success", "failure"]) {
  test(`connecting a new SSH host switches inside the app: ${outcome}`, async (t) => {
    t.mock.method(window, "open", () => assert.fail("no browser popup"));
    let finishConnection;
    const connections = [];
    await laptop("/projects", {
      "GET /api/remote/sessions": () => ({ sessions: [] }),
      "POST /api/remote/sessions": async (init) => {
        connections.push(JSON.parse(init.body));
        await new Promise((resolve) => { finishConnection = resolve; });
        return outcome === "failure" ? Response.json({ error: "connection failed" }, { status: 502 }) : server;
      },
    });
    await click(switcher());
    await click(menuItem(m.remote_dialog_title()));
    await click(find(`${visible} [role="dialog"] button`, "research"));
    assert.deepEqual(connections, [{ host: "research", uiPreferences: { theme: "system", locale: "en" } }]);
    const dialog = document.querySelector('[role="dialog"]');
    assert.ok(dialog.querySelector('[role="status"] [role="progressbar"]'));
    assert.ok([...dialog.querySelectorAll("button, input")].every((control) => control.disabled));

    await act(async () => finishConnection());
    await settle();
    assert.equal(document.querySelector('[role="progressbar"]')?.outerHTML, undefined);
    if (outcome === "success") {
      assert.deepEqual(dialogTitles(), []);
      assert.equal(visibleMachine(), "server");
    } else {
      assert.equal(document.querySelector('[role="dialog"] [role="alert"]').textContent, "connection failed");
      assert.equal(visibleMachine(), "local");
    }
  });
}

test("connection progress has a name and no invented percentage", async () => {
  const { container, rerender } = await render(createElement(ProgressBar, { label: "Connecting" }));
  const bar = () => container.querySelector('[role="progressbar"]');
  assert.equal(bar().getAttribute("aria-label"), "Connecting");
  assert.equal(bar().getAttribute("aria-valuenow"), null);

  await rerender(createElement(ProgressBar, { value: 30, max: 50 }));
  assert.equal(bar().getAttribute("aria-valuenow"), "60");
});

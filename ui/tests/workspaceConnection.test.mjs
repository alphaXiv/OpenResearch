import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { ProgressBar } from "../src/components/ProgressBar.tsx";
import { autoDir } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { find, press, render, renderApp, serveApp, settle, type } from "./render.mjs";

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
async function laptop(path, routes = {}, remote = () => server) {
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project", updatedAt: 1 }] }),
    "GET /api/projects/activity": () => ({ activity: [] }),
    "GET /api/remote/sessions": () => ({ sessions: [remote()] }),
    "GET /api/settings/ssh": () => ({ hosts: [{ host: "research" }, { host: "lab" }], defaultHost: null }),
    "GET /api/remote/hosts": () => ({ hosts: [{ host: "research" }] }),
    "POST /api/remote/sessions": () => remote(),
    ...routes,
  });
  const backend = globalThis.fetch;
  const requests = [];
  const gateway = "/_orx/workspaces/server";
  globalThis.fetch = async (input, init = {}) => {
    const url = new URL(String(input), location.href);
    requests.push(`${init.method ?? "GET"} ${url.pathname}`);
    if (!url.pathname.startsWith(`${gateway}/`)) return backend(input, init);
    if (url.pathname === `${gateway}/_orx/runtime`) return Response.json({ ...runtimes.ssh, session: remote() });
    return backend(url.pathname.slice(gateway.length) + url.search, init);
  };
  await renderApp(path);
  return requests;
}

for (const error of [null, "Connection to research closed."]) {
  test(`Disconnect keeps the machine listed, and selecting it connects again${error ? " after a lost connection" : ""}`, async () => {
    let remote = server;
    const connects = [];
    await laptop("/projects/p/tasks/new", {
      "POST /_orx/disconnect": () => {
        remote = { ...remote, status: "disconnected", error, canStartNewHost: false };
        return remote;
      },
      "POST /api/remote/sessions": (init) => {
        connects.push(JSON.parse(init.body).host);
        remote = server;
        return remote;
      },
    }, () => remote);
    await click(switcher());
    await click(menuItem("research"));
    assert.equal(visibleMachine(), "server");
    await click(switcher());
    await click(menuItem(m.remote_disconnect()));
    assert.equal(visibleMachine(), "local");

    await click(switcher());
    assert.match(menuItem("research").textContent, new RegExp(m.remote_machine_disconnected()));
    await click(menuItem("research"));
    assert.equal(visibleMachine(), "server");
    assert.deepEqual(connects, ["research", "research"]);
  });
}

test("a saved machine with no session is listed and connects when selected", async () => {
  let sessions = [];
  await laptop("/projects/p/tasks/new", {
    "GET /api/remote/sessions": () => ({ sessions }),
    "POST /api/remote/sessions": () => {
      sessions = [server];
      return server;
    },
  });
  await click(switcher());
  assert.match(menuItem("research").textContent, new RegExp(m.remote_machine_disconnected()));
  await click(menuItem("research"));
  assert.equal(visibleMachine(), "server");
});

test("Forget this machine drops it from the list and returns to the laptop", async () => {
  let hosts = [{ host: "research" }];
  let sessions = [server];
  const forgotten = [];
  await laptop("/projects/p/tasks/new", {
    "GET /api/remote/sessions": () => ({ sessions }),
    "GET /api/remote/hosts": () => ({ hosts }),
    "DELETE /api/remote/hosts/research": () => {
      forgotten.push("research");
      hosts = [];
      sessions = [];
      return { ok: true };
    },
  }, () => server);
  await click(switcher());
  await click(menuItem("research"));
  assert.equal(visibleMachine(), "server");

  await click(switcher());
  await click(menuItem(m.remote_forget_machine()));
  assert.deepEqual(forgotten, []);
  await click(find('[role="alertdialog"] button, [role="dialog"] button', m.remote_forget()));
  assert.deepEqual(forgotten, ["research"]);
  assert.equal(visibleMachine(), "local");
  await click(switcher());
  assert.equal(menuItem("research"), null);
});

test("a disconnected workspace without startup data offers Reconnect", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  let remote = server;
  let projectsAvailable = false;
  serveApp({
    "GET /_orx/runtime": () => ({ ...runtimes.ssh, session: remote }),
    "GET /api/projects": () => projectsAvailable
      ? { projects: [{ id: "p", name: "Project", updatedAt: 1 }] }
      : Response.json({ error: "connection unavailable" }, { status: 503 }),
    "POST /_orx/reconnect": () => {
      remote = server;
      projectsAvailable = true;
      return remote;
    },
  });
  await renderApp("/projects/p/tasks/new");
  remote = { ...remote, status: "disconnected", canStartNewHost: false };
  await act(async () => t.mock.timers.tick(2_000));
  await settle();
  await click(find("button", m.remote_reconnect()));
  await act(async () => t.mock.timers.tick(2_000));
  await settle();
  assert.ok(find("button", "Project"));
});

for (const [place, path] of [["home", "/projects"], ["sidebar", "/projects/p/tasks/new"]]) {
  test(`the ${place} machine menu switches between retained machines and lists the laptop's remotes from either`, async () => {
    const requests = await laptop(path);
    assert.equal(visibleMachine(), "local");
    await click(switcher());
    await click(menuItem("research"));
    assert.equal(visibleMachine(), "server");

    await click(switcher());
    await click(menuItem(m.remote_dialog_title()));
    assert.deepEqual(dialogTitles(), [m.remote_dialog_title()]);
    assert.ok(requests.includes("GET /api/remote/hosts"));
    assert.ok(!requests.some((request) => request.endsWith("/workspaces/server/api/remote/hosts")));
    await press("Escape");
    await settle();
    assert.deepEqual(dialogTitles(), []);

    await click(switcher());
    await click(menuItem(m.projects_local()));
    assert.equal(visibleMachine(), "local");
  });
}

for (const [status, shown] of [["connected", null], ["reconnecting", m.remote_machine_connecting()], ["disconnected", m.remote_machine_disconnected()]]) {
  test(`the sidebar names the machine it shows, ${status}`, async () => {
    const listed = { ...server, status };
    await laptop("/projects/p/tasks/new", {
      "GET /api/remote/sessions": () => ({ sessions: [listed] }),
      "POST /api/remote/sessions": () => listed,
    });
    assert.match(switcher().textContent, new RegExp(m.projects_local()));
    assert.equal(switcher().getAttribute("aria-label"), m.remote_machine_switcher({ machine: m.projects_local() }));

    await click(switcher());
    await click(menuItem("research"));
    assert.match(switcher().textContent, /research/);
    if (shown) {
      // Offline, the cloud and the name in amber stand in for the word.
      if (status === "disconnected") assert.doesNotMatch(switcher().textContent, new RegExp(shown));
      else assert.match(switcher().textContent, new RegExp(shown));
      assert.equal(switcher().getAttribute("aria-label"), m.remote_machine_switcher_status({ machine: autoDir("research"), status: shown }));
    } else {
      assert.equal(switcher().getAttribute("aria-label"), m.remote_machine_switcher({ machine: autoDir("research") }));
    }
  });
}

test("a compute-only machine is one click from Compute, on the laptop", async () => {
  await laptop("/projects/p/tasks/new");
  await click(switcher());
  await click(menuItem("research"));
  assert.equal(visibleMachine(), "server");

  await click(switcher());
  await click(menuItem(m.remote_dialog_title()));
  await click(find(`${visible} [role="dialog"] button`, m.remote_open_compute()));
  assert.deepEqual(dialogTitles(), []);
  assert.equal(visibleMachine(), "local");
  assert.equal(document.querySelector(`${visible} .chat-pane h1`)?.textContent, m.settings_page_settings());
  assert.equal(document.querySelector(`${visible} [role="tab"][aria-selected="true"]`)?.textContent, m.settings_page_compute());
});

test("a pasted ssh command is saved, then connected by the alias the server chose", async () => {
  const added = [];
  const connections = [];
  await laptop("/projects", {
    "GET /api/remote/sessions": () => ({ sessions: [] }),
    "POST /api/remote/hosts": (init) => {
      const { command } = JSON.parse(init.body);
      added.push(command);
      return command.includes("-Z")
        ? Response.json({ error: "Unknown ssh option -Z." }, { status: 400 })
        : { host: "gpu1.lab.edu" };
    },
    "POST /api/remote/sessions": (init) => {
      connections.push(JSON.parse(init.body).host);
      return server;
    },
  });
  await click(switcher());
  await click(menuItem(m.remote_dialog_title()));
  const field = document.querySelector(`[role="dialog"] input[aria-label="${m.remote_add_label()}"]`);
  assert.deepEqual([...document.querySelectorAll('[role="dialog"] datalist option')].map((option) => option.value), ["lab"]);

  await type(field, "ssh -Z gpu1");
  await click(find('[role="dialog"] button[type="submit"]', m.settings_connect()));
  assert.equal(document.querySelector('[role="dialog"] [role="alert"]').textContent, "Unknown ssh option -Z.");
  assert.deepEqual(connections, []);

  await type(field, "  ssh -p 2222 clo@gpu1.lab.edu  ");
  await click(find('[role="dialog"] button[type="submit"]', m.settings_connect()));
  assert.deepEqual(added, ["ssh -Z gpu1", "ssh -p 2222 clo@gpu1.lab.edu"]);
  assert.deepEqual(connections, ["gpu1.lab.edu"]);
  assert.deepEqual(dialogTitles(), []);
  assert.equal(visibleMachine(), "server");
});

test("a Forget the server refuses keeps the machine open and says why", async () => {
  let refuse;
  await laptop("/projects/p/tasks/new", {
    "DELETE /api/remote/hosts/research": () => new Promise((resolve) => {
      refuse = () => resolve(Response.json({ error: "Could not update the saved hosts file." }, { status: 500 }));
    }),
  });
  await click(switcher());
  await click(menuItem("research"));
  await click(switcher());
  await click(menuItem(m.remote_forget_machine()));
  await click(find('[role="dialog"] button', m.remote_forget()));
  assert.equal(visibleMachine(), "server");
  await act(async () => refuse());
  await settle();
  assert.equal(visibleMachine(), "server");
  assert.equal(document.querySelector('[role="dialog"] [role="alert"]')?.textContent, "Could not update the saved hosts file.");
});

test("saved machines that can't be read say why instead of listing none", async () => {
  await laptop("/projects/p/tasks/new", {
    "GET /api/remote/hosts": () => Response.json({ error: "Could not read the saved hosts file." }, { status: 500 }),
  });
  await click(switcher());
  assert.equal(find('[role="menu"] [role="alert"]')?.textContent, "Could not read the saved hosts file.");
});

test("a machine that fails to connect leaves focus on the switcher", async () => {
  await laptop("/projects/p/tasks/new", {
    "POST /api/remote/sessions": () => Response.json({ error: "Add this host in Connect to remote first." }, { status: 400 }),
  });
  await click(switcher());
  await click(menuItem("research"));
  assert.equal(visibleMachine(), "local");
  assert.ok(document.activeElement === switcher());
});

test("a saved remote is forgotten from Connect to remote, with or without a session", async () => {
  const removed = [];
  let hosts = [{ host: "research" }, { host: "lab", user: "me", hostname: "lab.example.edu", port: "2222" }];
  await laptop("/projects", {
    "GET /api/remote/sessions": () => ({ sessions: [server] }),
    "GET /api/remote/hosts": () => ({ hosts }),
    ...Object.fromEntries(["lab", "research"].map((host) => [`DELETE /api/remote/hosts/${host}`, () => {
      removed.push(host);
      hosts = hosts.filter((entry) => entry.host !== host);
      return { ok: true };
    }])),
  });
  await click(switcher());
  await click(menuItem(m.remote_dialog_title()));
  assert.ok(find('[role="dialog"] li', "me@lab.example.edu:2222"));
  const forgetButton = (host) => document.querySelector(`[role="dialog"] button[aria-label="${m.remote_forget_host({ host: autoDir(host) })}"]`);
  for (const host of ["lab", "research"]) {
    await click(forgetButton(host));
    await click(find('[role="alertdialog"] button, [role="dialog"] button', m.remote_forget()));
  }
  assert.deepEqual(removed, ["lab", "research"]);
  assert.equal(find('[role="dialog"] li', "lab"), null);
  assert.equal(find('[role="dialog"] li', "research"), null);
});

for (const kind of ["local", "ssh"]) {
  for (const scenario of ["loading", "error", "onboarding", "no-projects", "projects"]) {
    const offered = scenario === "error" || (kind === "ssh" && (scenario === "onboarding" || scenario === "no-projects"));
    test(`${kind} project home ${offered ? "offers" : "leaves"} machine selection ${offered ? "during" : "to the sidebar during"} ${scenario}`, async () => {
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
      if (!offered) {
        assert.equal(switcher(), undefined);
        return;
      }
      assert.equal(switcher().getAttribute("aria-label"), m.remote_machine_switcher({ machine: kind === "ssh" ? autoDir("research") : m.projects_local() }));
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

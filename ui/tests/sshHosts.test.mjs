import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { TARGET_LABELS } from "../src/computeTargets.ts";
import { m } from "../src/paraglide/messages.js";
import { find, renderApp, serveApp, settle, type } from "./render.mjs";

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};

async function openSsh(routes) {
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }),
    "GET /api/settings/compute": () => ({ defaultBackend: null, defaultFlavor: null, targets: [{ id: "ssh", configured: true, enabled: true, summary: "" }] }),
    "GET /api/engine/hosts": () => ({ hosts: [] }),
    ...routes,
  });
  await renderApp("/projects/p/settings/compute");
  await click(document.querySelector(`button[aria-label="${TARGET_LABELS.ssh()}"]`));
}

const addField = () => document.querySelector(`input[aria-label="${m.ssh_add_host_label()}"]`);

test("a compute host is added from a pasted ssh command, and only a saved one can be removed", async () => {
  let hosts = [{ host: "lab", hostname: "lab.example.edu" }];
  const added = [];
  const removed = [];
  await openSsh({
    "GET /api/settings/ssh": () => ({ hosts, defaultHost: null }),
    "POST /api/settings/ssh/hosts": (init) => {
      const { command } = JSON.parse(init.body);
      added.push(command);
      if (command.includes("lab")) {
        return Response.json({ error: "lab is already saved as a remote with different settings. Remove it first." }, { status: 409 });
      }
      hosts = [{ host: "gpu1", user: "me", port: "2222", saved: true }, ...hosts];
      return { host: "gpu1" };
    },
    "DELETE /api/settings/ssh/hosts/gpu1": () => {
      removed.push("gpu1");
      hosts = hosts.filter((host) => !host.saved);
      return { ok: true };
    },
  });

  await type(addField(), "ssh -p 2200 lab");
  await click(find("button", m.ssh_add_host()));
  assert.equal(find('[role="alert"]', "already saved")?.textContent, "lab is already saved as a remote with different settings. Remove it first.");
  assert.equal(addField().value, "ssh -p 2200 lab");

  await type(addField(), "ssh -p 2222 me@gpu1");
  await click(find("button", m.ssh_add_host()));
  assert.deepEqual(added, ["ssh -p 2200 lab", "ssh -p 2222 me@gpu1"]);
  assert.equal(addField().value, "");
  assert.equal(find('[role="alert"]', "already saved"), null);
  const gpu = document.querySelector('[data-ssh-host="gpu1"]');
  assert.ok(gpu.textContent.includes("me@gpu1:2222"));

  assert.equal(find('[data-ssh-host="lab"] button', m.ssh_remove_host()), null);
  await click(find('[data-ssh-host="gpu1"] button', m.ssh_remove_host()));
  assert.deepEqual(removed, ["gpu1"]);
  assert.equal(document.querySelector('[data-ssh-host="gpu1"]'), null);
});

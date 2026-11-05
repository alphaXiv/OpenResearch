import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { m } from "../src/paraglide/messages.js";
import { find, press, renderApp, serveApp, settle } from "./render.mjs";

/** Opens the settings page against a backend whose channel switches answer with `reply`. */
async function updates(releaseChannel = "stable", reply = (status) => status) {
  const requests = [];
  let status = {
    current: "0.2.21", latest: null, latestTag: null, channel: "app-bundle", releaseChannel, releaseChannelsAvailable: true,
    waitingForStable: false, waitingForCanary: false, selfUpdates: true, autoUpdate: true, envDisabled: false,
    updateAvailable: false, installedVersion: null, installedTag: null, restartRequired: false,
  };
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }),
    "GET /api/update": () => status,
    "POST /api/update/channel": (init) => {
      const { channel } = JSON.parse(init.body);
      requests.push(channel);
      const next = reply({ ...status, releaseChannel: channel });
      if (!(next instanceof Response)) status = next;
      return next;
    },
  });
  await renderApp("/projects/p/settings/settings");
  return {
    requests,
    channel: () => status.releaseChannel,
    radio: (channel) => document.querySelector(`[data-channel="${channel}"]`),
    dialog: () => document.querySelector('[role="dialog"]'),
  };
}

const click = async (element) => {
  await act(() => element.click());
  await settle();
};

test("switching to canary requires confirmation for pointer and keyboard", async () => {
  const view = await updates();
  await click(view.radio("canary"));
  assert.match(view.dialog().textContent, new RegExp(m.settings_canary_confirm_title()));
  assert.deepEqual(view.requests, []);
  await press("Escape");
  await settle();
  assert.equal(view.dialog(), null);
  assert.equal(view.channel(), "stable");

  const arrow = await press("ArrowRight", view.radio("stable"));
  await settle();
  assert.equal(arrow.defaultPrevented, true);
  assert.notEqual(view.dialog(), null);
  assert.deepEqual(view.requests, []);
  await click(find('[role="dialog"] button', m.settings_canary_confirm_action()));
  assert.deepEqual(view.requests, ["canary"]);
  assert.equal(view.channel(), "canary");
  assert.equal(view.dialog(), null);
});

test("switching back to stable does not require the canary confirmation", async () => {
  const view = await updates("canary");
  await click(view.radio("stable"));
  assert.deepEqual(view.requests, ["stable"]);
  assert.equal(view.channel(), "stable");
  assert.equal(view.dialog(), null);
});

test("a failed switch keeps the selected channel and reports the error", async () => {
  const view = await updates("stable", () => Response.json({ error: "Channel change failed" }, { status: 500 }));
  await click(view.radio("canary"));
  await click(find('[role="dialog"] button', m.settings_canary_confirm_action()));
  assert.deepEqual(view.requests, ["canary"]);
  assert.equal(view.channel(), "stable");
  assert.match(view.dialog().textContent, /Channel change failed/);
  assert.equal(find('[role="dialog"] button', m.settings_canary_confirm_action()).disabled, false);
});

import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { NetworkProxySection } from "../src/components/NetworkProxySection.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, render, serveApp, settle, type } from "./render.mjs";

const click = async (element) => {
  assert.ok(element);
  await act(async () => element.click());
  await settle();
};

test("network proxy settings save, show restart feedback, and clear to the environment default", async () => {
  let saved = { proxyUrl: null, restartRequired: false };
  serveApp({
    "GET /api/settings/network-proxy": () => saved,
    "POST /api/settings/network-proxy": (init) => {
      saved = { ...JSON.parse(init.body), restartRequired: true };
      return saved;
    },
  });
  await render(createElement(NetworkProxySection));
  await settle();
  const label = find("label", m.settings_proxy_address());
  assert.ok(label);
  const input = document.getElementById(label.htmlFor);
  assert.equal(input.type, "url");
  await type(input, "http://127.0.0.1:18080");
  await click(find("form button", m.common_save()));
  assert.equal(saved.proxyUrl, "http://127.0.0.1:18080");
  assert.ok(find('[role="status"]', m.settings_proxy_restart()));

  await type(input, "");
  await click(find("form button", m.common_save()));
  assert.equal(saved.proxyUrl, null);
});

test("a refused proxy save keeps the draft and displays the error inline", async () => {
  serveApp({
    "GET /api/settings/network-proxy": () => ({ proxyUrl: null, restartRequired: false }),
    "POST /api/settings/network-proxy": () => Response.json({ error: "Use an HTTP or HTTPS proxy address." }, { status: 400 }),
  });
  await render(createElement(NetworkProxySection));
  await settle();
  const label = find("label", m.settings_proxy_address());
  const input = document.getElementById(label.htmlFor);
  await type(input, "http://127.0.0.1:18081");
  await click(find("form button", m.common_save()));
  assert.ok(find('[role="alert"]', "HTTP or HTTPS"), document.body.textContent);
  assert.equal(input.value, "http://127.0.0.1:18081");
});

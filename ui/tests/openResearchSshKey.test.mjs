import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { OpenResearchSshKey } from "../src/components/OpenResearchSshKey.tsx";
import { Toaster } from "../src/components/ui/Toast.tsx";
import { m } from "../src/paraglide/messages.js";
import { render, serve, settle } from "./render.mjs";

const existing = { path: "~/.ssh/id_ed25519.pub", fingerprint: "SHA256:pU0aHZujtAnVqRHnpcOVxPe07Yft4bANE4UgHDCaYL4" };

async function step(sshKey, reply = () => ({ ok: true })) {
  const uploads = [];
  serve((path, init) => {
    if (path === "/api/settings/openresearch/ssh-key" && init.method === "POST") {
      uploads.push(path);
      return reply();
    }
  });
  const { container } = await render(createElement(OpenResearchSshKey, { sshKey }));
  return { container, uploads, button: () => container.querySelector("button") };
}

test("names the exact key and what uploading it does, with one primary button", async () => {
  const { container, button } = await step(existing);
  assert.match(container.textContent, new RegExp(existing.path));
  assert.match(container.textContent, new RegExp(existing.fingerprint));
  assert.match(container.textContent, new RegExp(m.settings_ssh_key_upload_note()));
  assert.doesNotMatch(container.textContent, new RegExp(m.settings_ssh_key_new()));
  assert.equal(container.querySelectorAll("button").length, 1);
  assert.match(button().textContent, new RegExp(m.settings_ssh_key_add()));
});

test("a missing key says adding creates it", async () => {
  const { container } = await step({ path: existing.path, fingerprint: null });
  assert.match(container.textContent, new RegExp(existing.path));
  assert.match(container.textContent, new RegExp(m.settings_ssh_key_new()));
});

test("adding uploads once per click and confirms with a toast", async () => {
  let finish;
  await render(createElement(Toaster));
  const { button, uploads } = await step(existing, () => new Promise((resolve) => { finish = resolve; }));
  await act(() => button().click());
  await settle();
  assert.equal(uploads.length, 1);
  assert.equal(button().disabled, true);
  assert.match(button().textContent, new RegExp(m.settings_ssh_key_adding()));

  finish({ ok: true });
  await settle();
  assert.match(document.body.textContent, new RegExp(m.settings_ssh_key_added()));
  assert.equal(uploads.length, 1);
});

test("a failure shows its reason and offers a retry", async () => {
  const reason = "SSH key setup timed out. Try again.";
  const { container, button } = await step(existing, () => Response.json({ error: reason }, { status: 504 }));
  await act(() => button().click());
  await settle();
  assert.equal(container.querySelector('[role="alert"]').textContent, reason);
  assert.match(button().textContent, new RegExp(m.app_retry()));
  assert.equal(button().disabled, false);
});

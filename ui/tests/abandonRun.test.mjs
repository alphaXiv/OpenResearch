import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { RunContactNotice } from "../src/components/RunContactNotice.tsx";
import { m } from "../src/paraglide/messages.js";
import { find, render, serve, settle } from "./render.mjs";

const dialog = () => document.querySelector('[role="dialog"], [role="alertdialog"]');
const click = async (element) => {
  await act(async () => element.click());
  await settle();
};

/** Opens Abandon on a cancelling run whose machine reports `contact`; the backend answers the abandonment with `abandon`. */
async function openAbandon(contact = { type: "unreachable", since: 1, cause: "Offline" }, abandon = () => ({ id: "run" })) {
  const abandoned = [];
  serve((path, init) => {
    if (path === "/api/compute/hosts") return [];
    if (path === "/api/runs/run/abandon" && init.method === "POST") {
      abandoned.push("run");
      return abandon();
    }
  });
  await render(createElement(RunContactNotice, { run: { id: "run", status: "running", cancelling: true, contact, host: "gpu-box", backend: null } }));
  await settle();
  await click(find("button", m.run_abandon()));
  return abandoned;
}

test("Abandon confirms first and warns about unreachable compute", async () => {
  const abandoned = await openAbandon();
  for (const text of [m.run_abandon_title(), m.run_abandon_unreachable(), m.run_abandon_effect()]) assert.ok(dialog().textContent.includes(text), text);
  assert.deepEqual(abandoned, []);
  await click(find("[role] button", m.run_abandon_confirm()));
  assert.deepEqual(abandoned, ["run"]);
  assert.equal(dialog()?.outerHTML, undefined);
});

test("keeping the run tracked abandons nothing", async () => {
  const abandoned = await openAbandon();
  await click(find("[role] button", m.run_abandon_keep()));
  assert.deepEqual(abandoned, []);
  assert.equal(dialog()?.outerHTML, undefined);
});

test("failed abandonment keeps the dialog open with the error", async () => {
  await openAbandon(undefined, () => Response.json({ error: "Host changed" }, { status: 409 }));
  await click(find("[role] button", m.run_abandon_confirm()));
  assert.ok(dialog().textContent.includes("Host changed"));
  assert.equal(find("[role] button", m.run_abandon_confirm()).disabled, false);
});

test("Abandon on a host that needs sign-in does not claim the machine is unreachable", async () => {
  await openAbandon({ type: "needs_auth", since: 1, cause: "Connect" });
  assert.ok(!dialog().textContent.includes(m.run_abandon_unreachable()));
  assert.ok(dialog().textContent.includes(m.run_abandon_effect()));
});

import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { RunContactNotice } from "../src/components/RunContactNotice.tsx";
import { fmtClock } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { render, serve, settle } from "./render.mjs";

const plain = (text) => text.replace(/[⁦-⁩]/g, "");
const unreachable = { type: "unreachable", since: 100, cause: "Operation timed out" };
const needsAuth = { type: "needs_auth", since: 200, cause: "Duo approval required" };
const unreconciled = (detail) => ({ type: "unreconciled", since: 300, detail });
const host = (fields) => ({ alias: "gpu-box", root: "/runs", status: "backoff", class: "key", sinceMs: 100, nextRetryMs: null, cause: null, ...fields });

/** The notice for a running `gpu-box` run with `extra` fields, over a backend whose engine hosts are `hosts`. */
async function notice(extra = {}, hosts = []) {
  const requests = [];
  serve((path, init) => {
    requests.push(`${init.method ?? "GET"} ${path}`);
    if (path === "/api/compute/hosts") return hosts;
    if (/^\/api\/runs\/run\/(cancel|cleanup)$/.test(path)) return { ok: true };
  });
  const run = { id: "run", status: "running", cancelling: false, contact: null, lostReason: null, host: "gpu-box", backend: null, ...extra };
  const { container } = await render(createElement(RunContactNotice, { run }));
  await settle();
  const status = container.querySelector('[role="status"]');
  return {
    container,
    requests,
    empty: container.innerHTML === "",
    lines: status ? [...status.querySelectorAll("p")].map((line) => plain(line.textContent)) : [],
    buttons: [...container.querySelectorAll("button")].map((button) => button.textContent),
    click: async (label) => {
      await act(async () => [...container.querySelectorAll("button")].find((button) => button.textContent === label).click());
      await settle();
    },
  };
}

test("a live run with nothing to add renders nothing", async () => {
  assert.equal((await notice()).empty, true);
  assert.equal((await notice({ status: "done" })).empty, true);
});

test("a legacy monitoring error renders as before, without actions", async () => {
  const view = await notice({ backend: { monitoringError: "Monitoring unavailable: no exit status." } });
  assert.deepEqual(view.lines, ["Monitoring unavailable: no exit status."]);
  assert.deepEqual(view.buttons, []);
});

test("unreachable names the host, its next try when backing off, and offers Retry for key hosts", async () => {
  const backingOff = await notice({ contact: unreachable }, [host({ nextRetryMs: 900 })]);
  assert.deepEqual(backingOff.lines, [`${plain(m.run_notice_unreachable({ host: "gpu-box", time: fmtClock(100) }))} ${m.run_notice_retry_at({ time: fmtClock(900) })}`, "Operation timed out"]);
  assert.deepEqual(backingOff.buttons, [m.app_retry(), m.run_abandon()]);
  await backingOff.click(m.app_retry());
  assert.ok(backingOff.requests.includes("POST /api/compute/hosts/gpu-box/retry"));

  assert.deepEqual((await notice({ contact: unreachable })).buttons, [m.run_abandon()]);
  for (const status of ["live", "connecting", "needs_auth"]) {
    assert.deepEqual((await notice({ contact: unreachable }, [host({ status })])).buttons, [m.run_abandon()], `no Retry while ${status}`);
  }
  assert.equal((await notice({ contact: unreachable, host: null })).lines[0], plain(m.run_notice_unreachable({ host: m.run_notice_the_host(), time: fmtClock(100) })));
});

test("needs auth offers Connect first", async () => {
  const view = await notice({ contact: needsAuth });
  assert.equal(view.lines[0], plain(m.run_notice_needs_auth({ host: "gpu-box" })));
  assert.deepEqual(view.buttons, [m.settings_connect(), m.run_abandon()]);
});

test("pending and unverified cancels say why", async () => {
  assert.deepEqual((await notice({ cancelling: true, contact: unreachable })).lines, [
    m.status_cancellation_pending(),
    `${plain(m.run_notice_cancel_pending({ host: "gpu-box", time: fmtClock(100) }))} ${m.run_notice_cancel_pending_later()}`,
  ]);
  assert.deepEqual((await notice({ cancelling: true, contact: needsAuth })).buttons, [m.settings_connect(), m.run_abandon()]);
  assert.deepEqual((await notice({ status: "submitting", cancelling: true, contact: unreconciled("No sandbox id") })).lines, [m.status_cancellation_pending(), m.run_notice_cancel_pending_launch()]);

  const unverified = await notice({ cancelling: true, contact: unreconciled("2 processes still running") });
  assert.deepEqual(unverified.lines, [m.status_cancelling(), "2 processes still running"]);
  assert.deepEqual(unverified.buttons, [m.run_stop_again(), m.run_abandon()]);
  await unverified.click(m.run_stop_again());
  assert.ok(unverified.requests.includes("POST /api/runs/run/cancel"));
});

test("an unconfirmed launch can only be abandoned", async () => {
  const view = await notice({ status: "submitting", contact: unreconciled("The app stopped before Modal returned a sandbox id.") });
  assert.equal(view.lines[0], m.run_notice_unconfirmed_launch());
  assert.deepEqual(view.buttons, [m.run_abandon()]);
});

test("an abandoned run's pending kill is stated, with no actions", async () => {
  const view = await notice({ status: "lost", lostReason: "abandoned", contact: unreconciled("Abandoned") });
  assert.deepEqual(view.lines, [plain(m.run_notice_abandoned({ host: "gpu-box" }))]);
  assert.deepEqual(view.buttons, []);
});

test("a finished run's log collection is stated, with the cause when it failed", async () => {
  const pending = await notice({ status: "done", logCollection: "pending" });
  assert.deepEqual(pending.lines, [plain(m.run_notice_log_pending({ host: "gpu-box" }))]);
  assert.deepEqual(pending.buttons, []);
  const failed = await notice({ status: "done", logCollection: "failed", logCollectionError: "No space left on device" });
  assert.deepEqual(failed.lines, [plain(m.run_notice_log_failed({ host: "gpu-box" })), "No space left on device"]);
});

test("a finished run whose result never reached the chat says why, in one line", async () => {
  const view = await notice({ status: "done", noticeError: "Chat harness is no longer configured" });
  assert.deepEqual(view.lines, [m.run_notice_post_failed({ error: "Chat harness is no longer configured" })]);
  assert.deepEqual(view.buttons, []);
});

test("Retry follows the connection of the run's own root", async () => {
  const hosts = [host({ root: "/a" }), host({ root: "/b", status: "live" })];
  const on = async (root) => (await notice({ contact: unreachable, backend: { kind: "engine", handle: { type: "Ssh", host: "gpu-box", root } } }, hosts)).buttons;
  assert.deepEqual(await on("/a"), [m.app_retry(), m.run_abandon()]);
  assert.deepEqual(await on("/b"), [m.run_abandon()]);
});

test("the message comes first and the actions follow it, outside it", async () => {
  const { container } = await notice({ contact: needsAuth });
  const status = container.querySelector('[role="status"]');
  const [first] = container.querySelectorAll("button");
  assert.equal(status.querySelector("button"), null);
  assert.ok(status.compareDocumentPosition(first) & Node.DOCUMENT_POSITION_FOLLOWING);
});

test("a cancel waiting on a backing-off host offers Retry beside Abandon", async () => {
  assert.deepEqual((await notice({ cancelling: true, contact: unreachable }, [host()])).buttons, [m.app_retry(), m.run_abandon()]);
});

test("a cancelled run with survivors names the host and offers Cleanup as Stop now", async () => {
  const detail = "Not fully stopped · 2 processes still running on gpu-box";
  const view = await notice({ status: "cancelled", contact: { type: "stopping", since: 300, count: 2 } });
  assert.ok(view.lines.join(" ").includes(detail));
  assert.deepEqual(view.buttons, [m.run_stop_now()]);
  await view.click(m.run_stop_now());
  assert.ok(view.requests.includes("POST /api/runs/run/cleanup"));
  assert.equal((await notice({ status: "cancelled", contact: null })).empty, true);
});

test("a cancelled tombstone retains its survivor proof while host authentication needs attention", async () => {
  const detail = "Not fully stopped · 2 processes still running on gpu-box";
  const view = await notice({ status: "cancelled", contact: { type: "stopping", since: 300, count: 2 } }, [host({ status: "needs_auth", class: "interactive", cause: "MFA" })]);
  assert.ok(view.lines.join(" ").includes(detail));
  assert.deepEqual(view.buttons, [m.settings_connect(), m.run_stop_now()]);
});

test("lost contact with a rented box warns about billing and Stop box cancels it", async () => {
  for (const contact of [unreachable, needsAuth]) {
    const view = await notice({ contact, backend: { kind: "openresearch_job" } });
    assert.ok(view.lines.includes(m.run_notice_rental_billing()));
    await view.click(m.run_stop_box());
    assert.ok(view.requests.includes("POST /api/runs/run/cancel"));
  }
  for (const extra of [
    { backend: { kind: "ssh_job" }, contact: unreachable },
    { backend: { kind: "openresearch_job" }, contact: null },
    { backend: { kind: "openresearch_job" }, contact: unreachable, status: "lost" },
    { backend: { kind: "openresearch_job" }, contact: needsAuth, status: "cancelled" },
  ]) {
    const view = await notice(extra);
    assert.ok(!view.lines.includes(m.run_notice_rental_billing()));
    assert.ok(!view.buttons.includes(m.run_stop_box()));
  }
});

test("API credential rejection gives provider recovery without SSH Connect", async () => {
  for (const provider of ["ray_job", "hf_job", "vercel_job", "modal_job", "k8s_job"]) {
    for (const backend of [
      { kind: "engine", handle: { type: "Api", provider } },
      { kind: "engine", handle: { type: "Legacy", descriptor: { kind: provider } } },
      { kind: provider },
    ]) {
      for (const cancelling of [false, true]) {
        const view = await notice({ contact: { ...needsAuth, cause: "HTTP 403" }, backend, cancelling });
        assert.ok(view.lines.includes(provider === "k8s_job" ? m.run_notice_cluster_auth() : m.run_notice_provider_auth()), view.lines.join(" | "));
        assert.deepEqual(view.buttons, [m.run_abandon()]);
      }
    }
  }
});

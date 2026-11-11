import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { ExperimentLineage } from "../src/components/ExperimentLineage.tsx";
import { ExperimentRunBanner } from "../src/components/ExperimentRunBanner.tsx";
import { buildHistoryNodes } from "../src/components/experimentLineageModel.ts";
import { RunContactNotice } from "../src/components/RunContactNotice.tsx";
import { SessionHoverCard } from "../src/components/SessionHoverCard.tsx";
import { StatusBadge, statusLabel } from "../src/components/StatusBadge.tsx";
import { fmtNumber, ltr } from "../src/i18n.ts";
import { setLocale } from "../src/locale.ts";
import { m } from "../src/paraglide/messages.js";
import { runDisplay } from "../src/runDisplay.ts";
import { render, serve, settle } from "./render.mjs";

const run = (extra = {}) => ({ id: "run", experimentId: "exp", status: "cancelled", cancelling: false, cancelUnconfirmed: null, contact: null, host: "gpu-box", resultMarkdown: null, backend: null, createdAt: 1, ...extra });
const experiment = { id: "exp", title: "SSH sandwich", slug: "ssh-sandwich", createdAt: 1, parentExperimentId: null };
const plain = (text) => text.replace(/[⁦-⁩]/g, "");
const ignore = () => {};

/** What each surface shows for `value`: its status badge (with its tooltip), contact notice, lineage node and chat hover card. */
async function surfaces(value) {
  serve((path) => {
    if (path === "/api/compute/hosts") return [];
    if (path === "/api/chat/sessions/chat/experiments") return { experiments: [{ experiment, projectName: "Research", currentRun: value }] };
  });
  const [node] = buildHistoryNodes([experiment], [value]);
  const shown = async (element) => {
    const { container } = await render(element);
    await settle();
    return container;
  };
  const card = document.body.appendChild(document.createElement("div"));
  const hover = await shown(createElement(SessionHoverCard, {
    session: { id: "chat", projectId: "project", title: "SSH sandwich", harness: "codex" },
    modelName: null, anchor: new DOMRect(), cardRef: { current: null }, cardId: "session-hover", container: card,
    onEnter: ignore, onLeave: ignore, onOpenExperiment: ignore, onReturnToRow: ignore, onContinue: ignore,
  }));
  const notice = await shown(createElement(RunContactNotice, { run: value }));
  const text = {
    badge: plain((await shown(createElement(StatusBadge, runDisplay(value)))).innerHTML),
    notice: plain(notice.textContent),
    lineage: plain((await shown(createElement(ExperimentLineage, { node, onSelect: ignore }))).textContent),
    hover: plain(card.textContent + hover.textContent),
  };
  card.remove();
  return { ...text, noticeStatus: notice.querySelector('[role="status"]') !== null };
}

const survivors = plain(statusLabel("not_fully_stopped", { count: 2, host: "gpu-box" }));
const states = [
  [run({ status: "running", cancelling: true }), m.status_cancelling(), true],
  [run({ status: "running", cancelling: true, contact: { type: "unreachable", since: 1, cause: "Offline" } }), m.status_cancellation_pending(), true],
  [run({ status: "running", cancelling: true, contact: { type: "needs_auth", since: 1, cause: "Connect" } }), m.status_cancellation_pending(), true],
  [run({ status: "submitting", cancelling: true }), m.status_cancellation_pending(), true],
  [run({ status: "submitting", cancelling: true, contact: { type: "unreconciled", since: 1, detail: "No handle" } }), m.status_cancellation_pending(), true],
  [run({ status: "running", cancelling: true, contact: { type: "unreconciled", since: 1, detail: "Stop pending" } }), m.status_cancelling(), true],
  [run(), m.status_cancelled(), false],
  [run({ contact: { type: "stopping", since: 1, count: 2 } }), survivors, true],
  [run({ cancelUnconfirmed: [], resultMarkdown: "Container processes couldn't be checked." }), m.status_cancelled_unconfirmed(), true],
];

test("each cancellation state agrees across the badge, notice, lineage and session hover card", async () => {
  for (const [value, label, hasNotice] of states) {
    const shown = await surfaces(value);
    for (const surface of ["badge", "lineage", "hover"]) assert.ok(shown[surface].includes(label), `${surface}: ${shown[surface]}`);
    assert.equal(shown.noticeStatus, hasNotice, label);
    if (hasNotice) assert.ok(shown.notice.includes(label), shown.notice);
    assert.equal(shown.notice.includes(m.run_stop_now()), value.contact?.type === "stopping");
    if (value.cancelUnconfirmed) assert.ok(shown.notice.includes(m.run_notice_cancel_uncovered()));
  }
});

test("pending cancellation offers Abandon", async () => {
  for (const contact of [{ type: "unreachable", since: 1, cause: "Offline" }, { type: "needs_auth", since: 1, cause: "Connect" }]) {
    assert.ok((await surfaces(run({ status: "running", cancelling: true, contact }))).notice.includes(m.run_abandon()));
  }
});

test("chat run banners distinguish pending and delivered cancels", async () => {
  const banner = async (value) => plain((await render(createElement(ExperimentRunBanner, { runs: [{ run: value, experimentName: "SSH sandwich" }] }))).container.textContent);
  for (const [contact, label, other] of [
    [null, m.status_cancelling(), m.status_cancellation_pending()],
    [{ type: "unreachable", since: 1, cause: "Offline" }, m.status_cancellation_pending(), m.status_cancelling()],
    [{ type: "needs_auth", since: 1, cause: "Connect" }, m.status_cancellation_pending(), m.status_cancelling()],
  ]) {
    const text = await banner(run({ status: "running", cancelling: true, contact }));
    assert.ok(text.includes(label), text);
    assert.ok(!text.includes(other), text);
  }
  assert.ok((await banner(run({ status: "submitting", cancelling: true }))).includes(m.status_cancellation_pending()));
});

test("unchecked-process notes use the structured proof after the result text is edited", async () => {
  for (const [unreadable, expected] of [[[], m.run_notice_cancel_uncovered()], [[789, 790], plain(m.run_notice_cancel_unreadable({ pids: "789, 790" }))]]) {
    const { notice } = await surfaces(run({ cancelUnconfirmed: unreadable, resultMarkdown: "Edited result" }));
    assert.ok(notice.includes(expected), notice);
    assert.ok(!notice.includes("Edited result"));
  }
});

test("survivor counts take the locale's plural form, with the count and host", async (t) => {
  t.after(() => setLocale("en"));
  for (const [locale, counts] of [["en", [[1, "one"], [2, "other"]]], ["ru", [[1, "one"], [2, "few"], [5, "many"], [21, "one"]]], ["ar", [[0, "zero"], [2, "two"], [3, "few"], [11, "many"], [100, "other"]]]]) {
    setLocale(locale);
    for (const [count, form] of counts) {
      const expected = m[`status_stopping_${form}`]({ count: ltr(fmtNumber(count)), host: ltr("gpu-box") });
      assert.equal(statusLabel("not_fully_stopped", { count, host: "gpu-box" }), expected, `${locale} ${count}`);
    }
  }
});

test("a starting run shows the backend's waiting reason as plain text until contact is lost", async () => {
  const reason = "0/4 nodes are available: <b>4 Insufficient nvidia.com/gpu</b>";
  const waiting = run({ status: "starting", waitingReason: reason });
  const { container } = await render(createElement(StatusBadge, runDisplay(waiting)));
  assert.equal(plain(container.textContent), `${m.status_starting()} · ${reason}`);
  assert.equal(container.querySelector("b"), null);
  assert.ok(plain(container.querySelector("[title]").getAttribute("title")).endsWith(reason));

  const banner = plain((await render(createElement(ExperimentRunBanner, { runs: [{ run: waiting, experimentName: "SSH sandwich" }] }))).container.textContent);
  assert.ok(banner.includes(reason), banner);

  const lost = await surfaces(run({ status: "starting", waitingReason: reason, contact: { type: "unreachable", since: 1, cause: "Offline" } }));
  assert.ok(!lost.badge.includes(reason), lost.badge);
});

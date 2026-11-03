import assert from "node:assert/strict";
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { runLabelsHarness } from "./runLabelsHarness.mjs";

const run = (extra = {}) => ({ id: "run", experimentId: "exp", status: "cancelled", cancelling: false, cancelUnconfirmed: null, contact: null, host: "gpu-box", resultMarkdown: null, backend: null, createdAt: 1, ...extra });
const experiment = { id: "exp", title: "SSH sandwich", slug: "ssh-sandwich", createdAt: 1, parentExperimentId: null };
const text = (html) => html.replace(/[\u2066-\u2069]/g, "");

function surfaces(locale, value) {
  const { load } = runLabelsHarness(locale, [{ experiment, projectName: "Research", currentRun: value }]);
  const { runDisplay } = load("runDisplay");
  const { StatusBadge } = load("components/StatusBadge");
  const { RunContactNotice } = load("components/RunContactNotice");
  const { ExperimentLineage } = load("components/ExperimentLineage");
  const [node] = load("components/experimentLineageModel").buildHistoryNodes([experiment], [value]);
  const { SessionHoverCard } = load("components/SessionHoverCard");
  const previousWindow = globalThis.window;
  globalThis.window = { innerWidth: 1075, innerHeight: 900 };
  let hover;
  try {
    hover = renderToStaticMarkup(React.createElement(SessionHoverCard, {
      session: { id: "chat", projectId: "project", title: "SSH sandwich", harness: "codex" },
      modelName: null, anchor: { x: 0, y: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 },
      cardRef: { current: null }, cardId: "session-hover", container: {},
    }));
  } finally {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  }
  return {
    hover,
    badge: renderToStaticMarkup(React.createElement(StatusBadge, runDisplay(value))),
    notice: renderToStaticMarkup(React.createElement(RunContactNotice, { run: value })),
    lineage: renderToStaticMarkup(React.createElement(ExperimentLineage, { node, onSelect: () => {} })),
  };
}

const states = [
  [run({ status: "running", cancelling: true }), "Cancelling…", true],
  [run(), "Cancelled", false],
  [run({ contact: { type: "stopping", since: 1, count: 2 } }), "Not fully stopped · 2 processes still running on gpu-box", true],
  [run({ cancelUnconfirmed: [], resultMarkdown: "Container processes couldn't be checked." }), "Cancelled · couldn&#x27;t confirm", true],
];

test("each cancellation state agrees across the badge, notice, lineage and session hover card", () => {
  for (const [value, label, hasNotice] of states) {
    const rendered = surfaces("en", value);
    assert.ok(text(rendered.badge).includes(label), rendered.badge);
    assert.ok(text(rendered.lineage).includes(label), rendered.lineage);
    assert.ok(text(rendered.hover).includes(label), rendered.hover);
    assert.equal(Boolean(rendered.notice), hasNotice);
    if (hasNotice) {
      assert.ok(text(rendered.notice).includes(label), rendered.notice);
      assert.match(rendered.notice, /role="status"/);
    }
    assert.equal(rendered.notice.includes("Stop now"), value.contact?.type === "stopping");
    if (value.cancelUnconfirmed) assert.match(rendered.notice, /In-container processes or processes outside the host scan couldn&#x27;t be checked/);
  }
});

test("Japanese unchecked-process notes use structured proof after result text is edited", () => {
  for (const [unreadable, expected] of [
    [[], "コンテナ内やホストのスキャン範囲外のプロセスは確認できませんでした。"],
    [[789, 790], "ホストがプロセス 789, 790 を調べられず、停止を確認できませんでした。"],
  ]) {
    const rendered = surfaces("ja", run({ cancelUnconfirmed: unreadable, resultMarkdown: "Edited result" }));
    assert.ok(text(rendered.notice).includes(expected), rendered.notice);
    assert.ok(!rendered.notice.includes("Edited result"));
    assert.match(rendered.notice, /role="status"/);
  }
});

test("Japanese and Russian survivor labels reach all four surfaces with the count and host", () => {
  for (const [locale, count, expected] of [
    ["ja", 1, "完全には停止していません · gpu-box で 1 個のプロセスがまだ実行中です"],
    ["ja", 2, "完全には停止していません · gpu-box で 2 個のプロセスがまだ実行中です"],
    ["ru", 1, "Остановлено не полностью · 1 процесс всё ещё работает на gpu-box"],
    ["ru", 2, "Остановлено не полностью · 2 процесса всё ещё работают на gpu-box"],
    ["ru", 5, "Остановлено не полностью · 5 процессов всё ещё работают на gpu-box"],
    ["ru", 21, "Остановлено не полностью · 21 процесс всё ещё работает на gpu-box"],
  ]) {
    for (const [surface, html] of Object.entries(surfaces(locale, run({ contact: { type: "stopping", since: 1, count } })))) {
      assert.ok(text(html).includes(expected), `${locale} ${surface}: ${html}`);
      assert.doesNotMatch(html, /Not fully stopped/);
    }
  }
});

test("all ten locales select grammatical process counts", () => {
  const examples = {
    en: [[1, "1 process still"], [2, "2 processes still"]],
    es: [[1, "1 proceso sigue"], [2, "2 procesos siguen"]],
    fa: [[1, "فرایند"], [2, "فرایند"]],
    hi: [[1, "प्रक्रिया अभी"], [2, "प्रक्रियाएँ अभी"]],
    ja: [[1, "1 個のプロセス"], [2, "2 個のプロセス"]],
    ko: [[1, "프로세스 1개"], [2, "프로세스 2개"]],
    pl: [[1, "1 proces nadal"], [2, "2 procesy nadal"], [5, "5 procesów nadal"], [12, "12 procesów nadal"], [22, "22 procesy nadal"]],
    ru: [[1, "1 процесс всё"], [2, "2 процесса всё"], [5, "5 процессов всё"], [11, "11 процессов всё"], [22, "22 процесса всё"]],
    "zh-CN": [[1, "1 个进程"], [2, "2 个进程"]],
    ar: [[1, "عملية واحدة"], [2, "عمليتان"], [3, "عمليات"], [11, "عملية"], [100, "عملية"]],
  };
  for (const [locale, counts] of Object.entries(examples)) {
    const { statusLabel } = runLabelsHarness(locale).load("components/StatusBadge");
    for (const [count, expected] of counts) {
      const label = text(statusLabel("not_fully_stopped", { count, host: "gpu-box" }));
      assert.ok(label.includes(expected), `${locale} ${count}: ${label}`);
      assert.ok(label.includes(new Intl.NumberFormat(locale).format(count)));
      assert.ok(label.includes("gpu-box"));
    }
  }
});

if (process.env.ORX_LABEL_FIXTURES) {
  const directory = process.env.ORX_LABEL_FIXTURES;
  mkdirSync(directory, { recursive: true });
  const assets = new URL("../dist/assets/", import.meta.url);
  const css = readdirSync(assets).filter((name) => name.startsWith("index-") && name.endsWith(".css")).map((name) => readFileSync(new URL(name, assets), "utf8")).join("\n");
  for (const locale of ["en", "ja", "ru", "ar"]) {
    const cards = states.map(([value]) => {
      const rendered = surfaces(locale, value);
      return `<section style="padding:16px 0;border-bottom:1px solid var(--border)"><div style="min-width:0;overflow:hidden">${rendered.badge}</div><div style="margin-top:12px">${rendered.notice}</div><div style="margin-top:12px">${rendered.lineage}</div></section>`;
    }).join("");
    for (const [index, [value]] of states.entries()) {
      const { hover } = surfaces(locale, value);
      writeFileSync(`${directory}/${locale}-hover-${index}.html`, `<!doctype html><html lang="${locale}"><meta charset="utf-8"><style>${css}</style><body style="background:var(--base);color:var(--text)">${hover}</body></html>`);
    }
    writeFileSync(`${directory}/${locale}.html`, `<!doctype html><html lang="${locale}" dir="${locale === "ar" ? "rtl" : "ltr"}"><meta charset="utf-8"><style>${css}</style><body style="overflow:auto;background:var(--base);color:var(--text);padding:20px"><main style="max-width:960px;margin:0 auto">${cards}</main></body></html>`);
  }
}

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { createElement } from "react";
import { Stats } from "../src/components/run/blocks/Stats.tsx";
import { m } from "../src/paraglide/messages.js";
import { render } from "./render.mjs";

const running = JSON.parse(readFileSync(new URL("./fixtures/runMonitor/running.json", import.meta.url), "utf8"));

test("page stats keep their label and explain a missing value until it arrives", async () => {
  const monitor = { ...running, latest: {}, versus: { step: {}, final: {} } };
  const view = await render(createElement(Stats, { monitor, names: ["loss"], live: true }));
  assert.equal(view.container.querySelector("dt").textContent, running.view.signals.loss.label);
  const pending = view.container.querySelector('[role="img"]');
  assert.equal(pending.getAttribute("aria-label"), m.run_value_pending());
  assert.equal(pending.tabIndex, 0);
  await view.rerender(createElement(Stats, { monitor: running, names: ["loss"], live: true }));
  assert.equal(view.container.querySelector("dt").textContent, running.view.signals.loss.label);
  assert.equal(view.container.querySelector(`[aria-label="${m.run_value_pending()}"]`), null);
});

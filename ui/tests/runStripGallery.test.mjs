import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { galleryFetch, groups } from "../gallery/backend.ts";
import { Gallery } from "../gallery/Gallery.tsx";
import { render, settle } from "./render.mjs";

// A chart sizes itself to its box, which happy-dom never lays out.
for (const size of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"]) {
  Object.defineProperty(HTMLElement.prototype, size, { configurable: true, get: () => 800 });
}

test("every run gallery specimen renders the composer output", async (t) => {
  const errors = t.mock.method(console, "error");
  globalThis.fetch = galleryFetch;
  const { container } = await render(createElement(Gallery));
  await settle();

  const entries = groups.flatMap(({ specimens }) => specimens);
  assert.equal(container.querySelectorAll("[data-specimen]").length, entries.length);
  for (const entry of entries) {
    const specimen = container.querySelector(`[data-specimen="${entry.id}"] [data-specimen-output]`);
    const banner = specimen?.querySelector("section[aria-label]");
    assert.ok(banner, entry.label);
    const sentence = banner.querySelector('[data-part="sentence"], button[aria-haspopup="dialog"][aria-expanded]')
      ?? [...banner.querySelectorAll('[role="status"]')].find((status) => status.textContent.trim());
    assert.ok(sentence?.textContent.trim(), entry.label);
    for (const chart of banner.querySelectorAll('svg[role="img"]')) {
      assert.ok(chart.querySelector("path, circle, rect"), `${entry.label}: chart draws no marks`);
    }
    if (entry.pin === "stage") {
      const card = specimen.querySelector('[role="dialog"]');
      assert.ok(card?.textContent.trim(), entry.label);
    }
    if (entry.pin === "eta" || entry.pin === "stop") {
      const trigger = entry.pin === "eta" ? '[role="img"] >' : 'button[aria-haspopup="dialog"] +';
      assert.ok(specimen.querySelector(`${trigger} [role="tooltip"]`)?.textContent.trim(), entry.label);
    }
  }
  assert.ok(errors.mock.calls.every(({ arguments: [message] }) => message !== "run view block failed to draw"));
});

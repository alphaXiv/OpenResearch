import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { act, createElement } from "react";
import { DesktopAppBanner } from "../src/components/DesktopAppBanner.tsx";
import { render } from "./render.mjs";

const RELEASES = "https://github.com/alphaXiv/OpenResearch/releases/latest";
const KEY = "desktop-app-banner-dismissed-on";

async function banner({ platform = "MacIntel" } = {}) {
  Object.defineProperty(navigator, "platform", { value: platform, configurable: true });
  const { container } = await render(createElement(DesktopAppBanner));
  return container;
}

afterEach(() => {
  localStorage.clear();
  delete window.__ORX_DESKTOP__;
});

test("links the download for the viewer's platform", async () => {
  const cases = [
    ["MacIntel", `${RELEASES}/download/OpenResearch.dmg`],
    ["Win32", `${RELEASES}/download/OpenResearch-Setup.exe`],
    ["Linux x86_64", `${RELEASES}/download/OpenResearch-x86_64.AppImage`],
    ["Linux aarch64", `${RELEASES}/download/OpenResearch-aarch64.AppImage`],
  ];
  for (const [platform, href] of cases) {
    const link = (await banner({ platform })).querySelector("a");
    assert.equal(link.getAttribute("href"), href, platform);
    assert.equal(link.getAttribute("target"), null, platform);
  }
});

test("falls back to the release page in a new tab", async () => {
  const link = (await banner({ platform: "FreeBSD amd64" })).querySelector("a");
  assert.equal(link.getAttribute("href"), RELEASES);
  assert.equal(link.getAttribute("target"), "_blank");
});

test("dismissing hides the banner until the next calendar day", async () => {
  const container = await banner();
  await act(() => container.querySelector("button").click());
  assert.equal(localStorage.getItem(KEY), new Date().toDateString());
  assert.equal(container.innerHTML, "");
  assert.equal((await banner()).innerHTML, "");

  localStorage.setItem(KEY, new Date(Date.now() - 24 * 60 * 60 * 1000).toDateString());
  assert.notEqual((await banner()).innerHTML, "");
});

test("never renders inside the desktop app", async () => {
  window.__ORX_DESKTOP__ = true;
  assert.equal((await banner()).innerHTML, "");
});

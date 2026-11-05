import test from "node:test";
import assert from "node:assert/strict";
import { chromium } from "playwright-core";
import { createServer } from "vite";

test("chat and artifact Markdown preserve links without loading external images", async () => {
  assert.ok(process.env.ORX_BROWSER_EXECUTABLE, "Set ORX_BROWSER_EXECUTABLE to a Chromium or Chrome executable");
  const server = await createServer({ configFile: false, logLevel: "silent", server: { host: "127.0.0.1", port: 0, strictPort: true } });
  await server.listen();
  let browser;
  try {
    browser = await chromium.launch({ headless: true, executablePath: process.env.ORX_BROWSER_EXECUTABLE });
    const page = await browser.newPage();
    const external = [];
    await page.route(/^https?:\/\/(?!127\.0\.0\.1)/, route => { external.push(route.request().url()); return route.abort(); });
    await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/tests/browser/markdownImages.html`, { waitUntil: "domcontentloaded" });
    for (const id of ["chat", "artifact"]) {
      const section = page.locator(`#${id}`);
      await section.getByRole("link", { name: "Remote" }).waitFor({ timeout: 15000 });
      if (id === "chat") await section.getByRole("button", { name: "Next image" }).click();
      await section.getByRole("link", { name: "Protocol relative" }).waitFor({ timeout: 15000 });
      if (id === "chat") await section.getByRole("button", { name: "Next image" }).click();
      await section.locator("img[alt=Local]").waitFor({ timeout: 15000 });
      assert.equal(await section.locator("img").count(), 1);
      assert.equal(await section.locator("img").getAttribute("alt"), "Local");
      assert.equal(await section.getByRole("link", { name: "HTTPS://example.com/no-alt.png" }).count(), 1);
    }
    assert.deepEqual(external, []);
  } finally {
    await browser?.close();
    await server.close();
  }
});

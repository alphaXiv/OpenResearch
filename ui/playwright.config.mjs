import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  workers: 1,
  retries: 0,
  timeout: 30_000,
  expect: { timeout: 10_000 },
  outputDir: "e2e-results/browser",
  reporter: [["list"], ["html", { outputFolder: "e2e-report", open: "never" }]],
  use: {
    baseURL: process.env.ORX_E2E_URL,
    browserName: "chromium",
    viewport: { width: 1440, height: 900 },
    locale: "en-US",
    trace: "on",
    screenshot: "on",
    video: "on",
  },
});

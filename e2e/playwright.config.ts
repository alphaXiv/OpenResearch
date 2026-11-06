import { defineConfig, devices } from '@playwright/test'

if (!process.env.ORX_E2E_BINARY) throw new Error('Run the suite with node e2e/run.ts, which builds orx and the dashboard first')

export default defineConfig({
  testDir: './tests',
  fullyParallel: true,
  // Each worker runs its own orx up and agents beside the browser.
  workers: process.env.CI ? 2 : 4,
  // A test that passes only on retry is a race; fix it instead.
  retries: 0,
  forbidOnly: !!process.env.CI,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: process.env.CI ? [['github'], ['html', { open: 'never' }]] : [['list'], ['html', { open: 'never' }]],
  use: {
    ...devices['Desktop Chrome'],
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
})

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { lint } from './lint.ts'

const flagged = (source: string) => lint('spec.ts', source).length

test('flags each way of hiding a race', () => {
  for (const source of [
    `await expect(async () => { await button.click() }).toPass()`,
    `await page.waitForTimeout(500)`,
    `await new Promise((resolve) => setTimeout(resolve, 500))`,
    `import { setTimeout } from 'node:timers/promises'`,
    `await page.goto('/', { waitUntil: 'networkidle' })`,
    `await button.click({ force: true })`,
    `test.describe.configure({ mode: 'serial' })`,
    `test.describe.serial('journey', () => {})`,
    `test.skip()`,
    `test.skip('a journey', async () => {})`,
    `test.describe.skip('journeys', () => {})`,
    `test.fixme()`,
  ])
    assert.equal(flagged(source), 1, source)
})

test('passes waits on state and conditional skips', () => {
  for (const source of [
    `await expect(page.getByText('done')).toBeVisible()`,
    `await expect.poll(() => count()).toBe(1)`,
    `await agent.reached('mid-reply')`,
    `setTimeout(() => controller.abort(), 1000)`,
    `test.skip(process.platform === 'win32', 'no Unix sockets')`,
    `const options = { force: true }`,
  ])
    assert.equal(flagged(source), 0, source)
})

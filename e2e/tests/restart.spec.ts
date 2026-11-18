import { readFileSync } from 'node:fs'
import type { Page } from '@playwright/test'
import type { Agent } from '../src/agent.ts'
import { SPEEDUP, type Orx } from '../src/orx.ts'
import { expect, send, test } from '../src/test.ts'

// A stopped turn's lease lapses 60 s after its last heartbeat and orx up looks for lapsed ones
// every 61 s, both divided by SPEEDUP.
const leaseExpiry = { timeout: (2 * 61_000) / SPEEDUP + 10_000 }

/** Opens a project on its own server with one finished turn and one the agent is holding. */
async function midTurn(orx: Orx, page: Page, agent: Agent) {
  await orx.onboard()
  const project = await orx.createProject('restart')
  agent.script('Summarize the baseline.', [{ say: 'The baseline trains in four minutes.' }])
  agent.script('Profile the data loader.', [{ say: 'Profiling the loader. ' }, { hold: 'profiling' }, { say: 'Never sent.' }])
  await project.open(page)
  await send(page, 'Summarize the baseline.')
  await expect(page.getByText('The baseline trains in four minutes.')).toBeVisible()
  await send(page, 'Profile the data loader.')
  await agent.reached('profiling')
  // Persisted with what the page shows; a crash before then would rightly lose it.
  await expect(page.getByText('Profiling the loader.')).toBeVisible()
}

test('when orx up quits mid-turn, the next start resumes the turn on its own', async ({ page, agent, launchOrx }) => {
  const orx = await launchOrx()
  await midTurn(orx, page, agent)

  agent.script('Profile the data loader.', [{ say: 'The loader spends most of its time decoding JPEGs.' }])
  await orx.restart('SIGTERM')
  await page.reload()
  await expect(page.getByText('The baseline trains in four minutes.')).toBeVisible()
  await expect(page.getByText('Profiling the loader.')).toBeVisible()
  await expect(page.getByText('The loader spends most of its time decoding JPEGs.')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Continue', exact: true })).toBeHidden()
  await expect(page.getByText('Never sent.')).toBeHidden()
})

test('a message queued behind a turn survives orx up quitting and runs once the turn ends', async ({ page, agent, launchOrx }) => {
  const orx = await launchOrx()
  await midTurn(orx, page, agent)
  agent.script('Then try pinned memory.', [{ say: 'Pinned memory saves a second per epoch.' }])
  await send(page, 'Then try pinned memory.')
  await expect(page.getByText('Queued', { exact: true })).toBeVisible()

  agent.script('Profile the data loader.', [{ say: 'The loader spends most of its time decoding JPEGs.' }])
  await orx.restart('SIGTERM')
  await page.reload()
  await expect(page.getByText('Then try pinned memory.')).toBeVisible()
  await expect(page.getByText('The loader spends most of its time decoding JPEGs.')).toBeVisible()
  await expect(page.getByText('Pinned memory saves a second per epoch.')).toBeVisible(leaseExpiry)
  await expect(page.getByText('Queued', { exact: true })).toBeHidden()
  await expect(page.getByRole('button', { name: 'Continue', exact: true })).toBeHidden()
})

test('a crashed ACP turn finishes the same reply without another prompt', async ({ page, agent, launchOrx }) => {
  const orx = await launchOrx()
  await midTurn(orx, page, agent)
  const message = page.locator('[data-message-id]').filter({ has: page.getByText('Profiling the loader.', { exact: true }) })
  const messageId = await message.getAttribute('data-message-id')
  expect(messageId).toBeTruthy()
  await orx.restart('SIGKILL')
  await page.reload()
  agent.release('profiling')
  await expect(page.getByText('Profiling the loader. Never sent.', { exact: true })).toBeVisible()
  await expect(page.getByText(/before this turn finished/)).toBeHidden()
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeHidden()
  await expect(page.locator(`[data-message-id="${messageId}"]`).getByText('Profiling the loader. Never sent.', { exact: true })).toBeVisible()
  await page.reload()
  await expect(page.getByText('Profiling the loader. Never sent.', { exact: true })).toHaveCount(1)
})

test('an ACP permission card survives a crash and answers its original request', async ({ page, agent, launchOrx }) => {
  const orx = await launchOrx()
  await orx.onboard()
  const project = await orx.createProject('restart-permission')
  agent.script('Inspect the data.', [{ permission: 'Read the training data' }, { say: 'Data inspection finished.' }])
  await project.open(page)
  await send(page, 'Inspect the data.')
  await expect(page.getByRole('button', { name: 'Allow this action', exact: true })).toBeVisible()
  await orx.restart('SIGKILL')
  await page.reload()
  await expect(page.getByRole('button', { name: 'Allow this action', exact: true })).toHaveCount(1)
  await page.getByRole('button', { name: 'Allow this action', exact: true }).click()
  await expect(page.getByText('Data inspection finished.', { exact: true })).toBeVisible()
  await expect(page.getByText(/before this turn finished/)).toBeHidden()
})

test('an ACP command running during a crash completes on the adopted connection', async ({ page, agent, launchOrx }) => {
  const orx = await launchOrx()
  await orx.onboard()
  const project = await orx.createProject('restart-command')
  agent.script('Run the analysis.', [
    { say: 'Analysis started. ' },
    { run: `curl --fail --silent --request POST '${agent.url}/hold/command' && printf COMMAND_FINISHED` },
    { say: 'Analysis finished.' },
  ])
  await project.open(page)
  await send(page, 'Run the analysis.')
  await agent.reached('command')
  await orx.restart('SIGKILL')
  await page.reload()
  agent.release('command')
  await expect(page.getByText('Analysis finished.', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeHidden()
  await expect(page.getByText(/before this turn finished/)).toBeHidden()
  expect(readFileSync(orx.agentLog, 'utf8')).toContain('COMMAND_FINISHED')
})

test('a turn running when orx up hands off to its next build finishes the same reply', async ({ page, agent, launchOrx }) => {
  const orx = await launchOrx()
  await midTurn(orx, page, agent)
  // What `node dev.ts restart` and an update restart do: hand the work to the next server.
  await orx.restart('SIGUSR2')
  await page.reload()
  agent.release('profiling')
  await expect(page.getByText('Profiling the loader. Never sent.', { exact: true })).toBeVisible()
  await expect(page.getByText(/before this turn finished/)).toBeHidden()
  await page.reload()
  await expect(page.getByText('Profiling the loader. Never sent.', { exact: true })).toHaveCount(1)
  await expect(page.getByText('The baseline trains in four minutes.')).toHaveCount(1)
})

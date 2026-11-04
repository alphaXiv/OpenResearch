import type { Page } from '@playwright/test'
import type { Agent } from '../src/agent.ts'
import { SPEEDUP, type Orx } from '../src/orx.ts'
import { expect, send, test } from '../src/test.ts'

// A crashed turn's lease lapses 60 s after its last heartbeat and orx up looks for lapsed ones
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

for (const [signal, how, quitting] of [['SIGTERM', 'quits', 'quitting'], ['SIGKILL', 'crashes', 'crashing']] as const) {
  test(`when orx up ${how} mid-turn, the transcript survives and Continue resumes the turn`, async ({ page, agent, launchOrx }) => {
    const orx = await launchOrx()
    await midTurn(orx, page, agent)

    await orx.restart(signal)
    await page.reload()
    await expect(page.getByText('The baseline trains in four minutes.')).toBeVisible()
    await expect(page.getByText('Profiling the loader.')).toBeVisible()
    await expect(page.getByText('This turn did not finish.')).toBeVisible(leaseExpiry)
    await expect(page.getByText('Never sent.')).toBeHidden()

    agent.script('Profile the data loader.', [{ say: 'The loader spends most of its time decoding JPEGs.' }])
    await page.getByText('This turn did not finish.').click()
    await page.getByRole('button', { name: 'Continue', exact: true }).click()
    await expect(page.getByText('The loader spends most of its time decoding JPEGs.')).toBeVisible()
    await expect(page.getByText('This turn did not finish.')).toBeHidden()
  })

  test(`a message queued behind a turn survives orx up ${quitting} and runs once the turn is given up`, async ({ page, agent, launchOrx }) => {
    const orx = await launchOrx()
    await midTurn(orx, page, agent)
    agent.script('Then try pinned memory.', [{ say: 'Pinned memory saves a second per epoch.' }])
    await send(page, 'Then try pinned memory.')
    await expect(page.getByText('Queued', { exact: true })).toBeVisible()

    await orx.restart(signal)
    await page.reload()
    await expect(page.getByText('Then try pinned memory.')).toBeVisible()
    await expect(page.getByText('This turn did not finish.')).toBeVisible(leaseExpiry)
    await expect(page.getByText('Pinned memory saves a second per epoch.')).toBeVisible()
    await expect(page.getByText('This turn did not finish.')).toHaveCount(1)
    await expect(page.getByText('Queued', { exact: true })).toBeHidden()
  })
}

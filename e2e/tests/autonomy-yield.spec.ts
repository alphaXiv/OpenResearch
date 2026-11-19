import type { Page } from '@playwright/test'
import { composer, expect, send, test } from '../src/test.ts'

type Yield = { kind: 'running' | 'need' | 'done'; text: string }

/**
 * Serves the chat's latest turn as closed by `turnYield`, as a host with the yield tool reports it.
 * The fake agent has no orx tools, so the fields ride on its real transcript and session list.
 */
async function closeLatestTurn(page: Page, sessionId: string, turnYield: Yield) {
  await page.unrouteAll({ behavior: 'wait' })
  await page.route(`**/api/chat/sessions/${sessionId}/messages*`, async (route) => {
    const response = await route.fetch()
    const body = await response.json()
    const last = [...body.messages].reverse().find((message: { role: string }) => message.role === 'assistant')
    if (last) last.turnYield = turnYield
    await route.fulfill({ response, json: body })
  })
  await page.route('**/api/chat/sessions?*', async (route) => {
    const response = await route.fetch()
    const body = await response.json()
    for (const session of body.sessions ?? []) {
      if (session.id !== sessionId) continue
      session.yieldKind = turnYield.kind
      session.yieldText = turnYield.text
    }
    await route.fulfill({ response, json: body })
  })
}

test("the composer shows why the agent stopped until a reply starts a new turn", async ({ page, project, agent }, info) => {
  await page.addInitScript(() => localStorage.setItem('sidebar-grouping', 'inbox'))
  await page.emulateMedia({ reducedMotion: 'reduce' })
  agent.script('Compare the fine-tune with Wispr.', [{ say: 'The fine-tune reaches 3.21% WER on the held-out set, against 2.40% for Wispr.' }])
  await project.open(page)
  await send(page, 'Compare the fine-tune with Wispr.')
  const answer = page.getByText('The fine-tune reaches 3.21% WER', { exact: false })
  await expect(answer).toBeVisible()
  const sessionId = new URL(page.url()).pathname.split('/').at(-1)!

  const closes: Yield[] = [
    { kind: 'running', text: 'Two seeds of the larger adapter are still training; their results restart the comparison.' },
    { kind: 'need', text: 'Share the 40 recordings Wispr was scored on, so both models are compared on the same audio.' },
    { kind: 'done', text: 'The comparison is finished within the four GPU-hour budget.' },
  ]
  const labels = { running: 'Waiting on results', need: 'Needs you', done: 'Done' }
  for (const close of closes) {
    await test.step(`a ${close.kind} close`, async () => {
      await closeLatestTurn(page, sessionId, close)
      await page.reload()
      const line = page.locator('.composer').getByRole('status').filter({ hasText: close.text })
      await expect(line.getByText(close.text)).toBeVisible()
      await expect(line.getByText(labels[close.kind], { exact: true })).toBeVisible()
      await expect(page.getByRole('feed').getByText(close.text)).toHaveCount(0)

      const row = page.locator('.session-row', { hasText: 'Compare the fine-tune' })
      if (close.kind === 'need') {
        await expect(page.getByRole('button', { name: 'Inbox', exact: true })).toHaveAttribute('aria-expanded', 'true')
        await expect(row).toHaveAccessibleName(/Waiting for your input/)
        await expect(row.getByText(close.text)).toBeVisible()
      } else {
        await expect(row).not.toHaveAccessibleName(/Waiting for your input/)
      }
      for (const colorScheme of ['light', 'dark'] as const) {
        await page.emulateMedia({ colorScheme })
        await expect(page.locator('html')).toHaveAttribute('data-theme', colorScheme)
        await page.screenshot({ path: info.outputPath(`close-${close.kind}-${colorScheme}.png`), animations: 'disabled' })
      }
      await page.emulateMedia({ colorScheme: 'light' })
    })
  }

  await test.step('a narrow window wraps the line', async () => {
    await page.setViewportSize({ width: 720, height: 640 })
    await expect(page.locator('[data-turn-close]')).toBeVisible()
    await page.screenshot({ path: info.outputPath('close-done-narrow.png'), animations: 'disabled' })
  })

  await test.step('a reply clears the closing line', async () => {
    await page.unrouteAll({ behavior: 'wait' })
    agent.script('Continue with the next trial.', [{ say: 'The next trial is ready for review.' }])
    await send(page, 'Continue with the next trial.')
    await expect(page.locator('[data-turn-close]')).toBeHidden()
    await expect(page.getByText('The next trial is ready for review.')).toBeVisible()
  })
})

/** Serves the chat's latest reply as ending on `parts`, the way a host that recorded them reports it. */
async function endLatestTurnWith(page: Page, sessionId: string, parts: () => object[], awaitingInput: () => boolean) {
  await page.unrouteAll({ behavior: 'wait' })
  await page.route(`**/api/chat/sessions/${sessionId}/messages*`, async (route) => {
    const response = await route.fetch()
    const body = await response.json()
    const last = [...body.messages].reverse().find((message: { role: string }) => message.role === 'assistant')
    if (last) last.parts = [...last.parts, ...parts()]
    await route.fulfill({ response, json: body })
  })
  await page.route('**/api/chat/sessions?*', async (route) => {
    const response = await route.fetch()
    const body = await response.json()
    for (const session of body.sessions ?? []) if (session.id === sessionId) session.awaitingInput = awaitingInput()
    await route.fulfill({ response, json: body })
  })
}

test('a plan shows its input status until it is accepted', async ({ page, project, agent }, info) => {
  await page.addInitScript(() => localStorage.setItem('sidebar-grouping', 'inbox'))
  await page.emulateMedia({ reducedMotion: 'reduce' })
  agent.script('Plan the comparison with Wispr.', [{ say: 'Compare both models on the same 40 recordings, then report WER per speaker.' }])
  await project.open(page)
  await send(page, 'Plan the comparison with Wispr.')
  await expect(page.getByText('Compare both models on the same 40 recordings', { exact: false })).toBeVisible()
  const sessionId = new URL(page.url()).pathname.split('/').at(-1)!

  let resolved = false
  const plan = 'Compare both models on the same 40 recordings, then report WER per speaker.'
  await endLatestTurnWith(page, sessionId, () => [{ id: 'plan-card', type: 'prompt', prompt: { kind: 'plan', plan, synthesized: true, resolved, ...(resolved ? { approved: true } : {}) } }], () => !resolved)
  await page.route(`**/api/chat/sessions/${sessionId}/respond`, async (route) => {
    resolved = true
    await route.fulfill({ json: { ok: true } })
  })
  await page.reload()

  const row = page.locator('.session-row', { hasText: 'Plan the comparison' })
  await expect(row.getByText('Needs input', { exact: true })).toBeVisible()
  await expect(row).toHaveAccessibleName(/Waiting for your input/)
  for (const colorScheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme })
    await expect(page.locator('html')).toHaveAttribute('data-theme', colorScheme)
    await page.screenshot({ path: info.outputPath(`plan-needs-you-${colorScheme}.png`), animations: 'disabled' })
  }
  await page.emulateMedia({ colorScheme: 'light' })

  await page.getByRole('button', { name: 'Accept plan' }).click()
  await expect(row.getByText('Needs input', { exact: true })).toBeHidden()
  await expect(row).not.toHaveAccessibleName(/Waiting for your input/)
})

test('a turn that stopped before its closing line keeps the yield call in its activity', async ({ page, project, agent }, info) => {
  await page.emulateMedia({ reducedMotion: 'reduce' })
  agent.script('Compare the fine-tune with Wispr.', [{ say: 'The fine-tune reaches 3.21% WER on the held-out set, against 2.40% for Wispr.' }])
  await project.open(page)
  await send(page, 'Compare the fine-tune with Wispr.')
  await expect(page.getByText('The fine-tune reaches 3.21% WER', { exact: false })).toBeVisible()
  const sessionId = new URL(page.url()).pathname.split('/').at(-1)!

  const text = 'Share the 40 recordings Wispr was scored on, so both models are compared on the same audio.'
  await endLatestTurnWith(page, sessionId, () => [{ id: 'yield-call', type: 'tool', tool: 'mcp__orx__yield', state: { status: 'completed', input: { kind: 'need', text } } }], () => false)
  await page.reload()

  await expect(page.locator('[data-part-id="yield-call"]')).toHaveText(`Needs you: ${text}`)
  await expect(page.locator('[data-turn-close]')).toHaveCount(0)
  for (const colorScheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme })
    await expect(page.locator('html')).toHaveAttribute('data-theme', colorScheme)
    await page.screenshot({ path: info.outputPath(`stopped-yield-${colorScheme}.png`), animations: 'disabled' })
  }
})

test('the Goal chip opens the goal to edit; Escape returns to the chip and Clear to the composer', async ({ page, project, agent }, info) => {
  await page.emulateMedia({ reducedMotion: 'reduce' })
  agent.script('Plan the comparison.', [{ say: 'I will compare the adapters on the held-out set.' }])
  await project.open(page)
  await send(page, 'Plan the comparison.')
  await expect(page.getByText('I will compare the adapters on the held-out set.')).toBeVisible()
  const sessionId = new URL(page.url()).pathname.split('/').at(-1)!
  const origin = new URL(page.url()).origin
  const goal = 'Beat 2.40% WER on the held-out set within four GPU-hours.'
  const patched = await page.request.patch(`/api/chat/sessions/${sessionId}`, { data: { goal }, headers: { origin } })
  expect(patched.ok()).toBe(true)

  const chip = page.getByRole('button', { name: /^Edit the goal: / })
  await expect(chip).toHaveAccessibleName(/within four GPU-hours/)
  await chip.click()
  const dialog = page.getByRole('dialog', { name: 'Edit goal' })
  const field = dialog.getByRole('textbox', { name: 'Goal' })
  await expect(field).toHaveValue(goal)
  await expect(dialog.getByRole('status')).toHaveCount(1)
  for (const colorScheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme })
    await expect(page.locator('html')).toHaveAttribute('data-theme', colorScheme)
    await page.screenshot({ path: info.outputPath(`goal-dialog-${colorScheme}.png`), animations: 'disabled' })
  }
  await page.keyboard.press('Escape')
  await expect(dialog).toBeHidden()
  await expect(chip).toBeFocused()

  await chip.press('Enter')
  await field.fill('Beat 2.40% WER on the held-out set within six GPU-hours.')
  const changed = await page.request.patch(`/api/chat/sessions/${sessionId}`, { data: { goal: 'Stop at two GPU-hours.' }, headers: { origin } })
  expect(changed.ok()).toBe(true)
  await expect(dialog.getByRole('status')).toContainText('The goal changed while you were editing.')
  await expect(field).toHaveValue('Beat 2.40% WER on the held-out set within six GPU-hours.')
  await page.screenshot({ path: info.outputPath('goal-dialog-changed-dark.png'), animations: 'disabled' })
  await dialog.getByRole('button', { name: 'Save', exact: true }).click()
  await expect(dialog).toBeHidden()
  await expect(chip).toHaveAccessibleName(/within six GPU-hours/)

  await chip.click()
  await dialog.getByRole('button', { name: 'Clear goal' }).click()
  await expect(dialog).toBeHidden()
  await expect(chip).toBeHidden()
  await expect(composer(page)).toBeFocused()
})

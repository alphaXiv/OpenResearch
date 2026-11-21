import { composer, expect, send, test } from '../src/test.ts'

test('a reply streams in as the agent writes it and is still there after a reload', async ({ page, project, agent }) => {
  agent.script('Which optimizer should the baseline use?', [
    { think: 'Comparing optimizers.' },
    { say: 'Start with AdamW at 3e-4. ' },
    { hold: 'mid-reply' },
    { say: 'Sweep the learning rate once the baseline runs.' },
  ])
  await project.open(page)
  await composer(page).fill('Which optimizer should the baseline use?')
  await page.getByRole('button', { name: 'Send', exact: true }).click()

  await test.step('the first half shows while the agent is still writing', async () => {
    await agent.reached('mid-reply')
    await expect(page.getByText('Start with AdamW at 3e-4.')).toBeVisible()
    await expect(page.getByText('Sweep the learning rate')).toBeHidden()
    await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible()
  })

  await test.step('the rest arrives and the turn ends', async () => {
    agent.release('mid-reply')
    await expect(page.getByText('Start with AdamW at 3e-4. Sweep the learning rate once the baseline runs.')).toBeVisible()
    await expect(page.getByRole('button', { name: 'Stop' })).toBeHidden()
  })

  await test.step('a reload shows the same transcript', async () => {
    await page.reload()
    await expect(page.getByText('Start with AdamW at 3e-4. Sweep the learning rate once the baseline runs.')).toBeVisible()
    await expect(page.getByRole('region', { name: 'Which optimizer should the baseline use?' })).toContainText('Which optimizer should the baseline use?')
  })
})

test('a message sent during a turn waits in the queue and runs when the turn ends', async ({ page, project, agent }) => {
  agent.script('Write the data loader.', [{ say: 'Writing the loader. ' }, { hold: 'busy' }, { say: 'The loader is done.' }])
  agent.script('Then add a validation split.', [{ say: 'Added a 10% validation split.' }])
  await project.open(page)
  await send(page, 'Write the data loader.')
  await agent.reached('busy')

  await send(page, 'Then add a validation split.')
  const queued = page.getByText('Queued', { exact: true })
  await expect(queued).toBeVisible()
  await expect(page.getByRole('button', { name: 'Remove queued message' })).toBeVisible()

  agent.release('busy')
  await expect(page.getByText('Added a 10% validation split.')).toBeVisible()
  await expect(queued).toBeHidden()
  const transcript = page.getByRole('region', { name: 'Write the data loader.' })
  await expect(transcript).toContainText(/The loader is done\.[\s\S]*Then add a validation split\.[\s\S]*Added a 10% validation split\./)
})

test('Stop ends the turn and the queued messages run next, in order', async ({ page, project, agent }) => {
  agent.script('Train for ten epochs.', [{ say: 'Epoch 1 of 10. ' }, { hold: 'training' }, { say: 'Epoch 2 of 10.' }])
  agent.script('Plot the loss.', [{ say: 'Plotted the loss.' }])
  agent.script('Then save a checkpoint.', [{ say: 'Saved the checkpoint.' }])
  await project.open(page)
  await send(page, 'Train for ten epochs.')
  await agent.reached('training')
  await send(page, 'Plot the loss.')
  await send(page, 'Then save a checkpoint.')
  const queued = page.getByText('Queued', { exact: true })
  await expect(queued).toHaveCount(2)

  await page.getByRole('button', { name: 'Stop' }).click()
  await expect(page.getByText('Saved the checkpoint.')).toBeVisible()
  await expect(queued).toHaveCount(0)
  await expect(page.getByText('Epoch 2 of 10.')).toBeHidden()
  const transcript = page.getByRole('region', { name: 'Train for ten epochs.' })
  await expect(transcript).toContainText(/Epoch 1 of 10\.[\s\S]*Plot the loss\.[\s\S]*Plotted the loss\.[\s\S]*Then save a checkpoint\.[\s\S]*Saved the checkpoint\./)
})

test('a drag that starts beside a reply selects from the line it started on', async ({ page, project, agent }) => {
  const reply = 'The stock model handles real whispers and fan noise far better than expected. Its worst problem is writing text on silence and noise. The synthetic whispers are harsher than the real ones, so that suite is miscalibrated and needs a new baseline before the augmented runs can be compared against it.'
  agent.script('How did the baseline do?', [{ say: reply }])
  await project.open(page)
  await send(page, 'How did the baseline do?')
  const paragraph = page.getByText(reply)
  await expect(paragraph).toBeVisible()

  const text = await paragraph.boundingBox()
  const pane = await page.getByRole('region', { name: 'How did the baseline do?' }).boundingBox()
  if (!text || !pane) throw new Error('The reply or the transcript has no box')
  const lineHeight = await paragraph.evaluate((element) => Number.parseFloat(getComputedStyle(element).lineHeight))
  const beside = { x: (pane.x + text.x) / 2, y: text.y + lineHeight * 1.5 }

  // WebKit anchors a press that lands outside every row at the start of the transcript.
  const pressed = await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.closest('[role=article]')?.textContent ?? '', beside)
  expect(pressed).toContain(reply)

  await page.mouse.move(beside.x, beside.y)
  await page.mouse.down()
  await page.mouse.move(text.x + text.width / 2, text.y + lineHeight * 2.5, { steps: 5 })
  await page.mouse.up()
  const firstLine = await page.evaluate(() => {
    const selection = getSelection()
    return selection?.rangeCount ? selection.getRangeAt(0).getClientRects()[0]?.toJSON() : undefined
  })
  expect(firstLine?.top).toBeLessThanOrEqual(beside.y)
  expect(firstLine?.bottom).toBeGreaterThanOrEqual(beside.y)
  expect(firstLine?.left).toBeCloseTo(text.x, 0)
})

test("a chat's hover card holds for a pointer crossing the rows below on its way, and leaves with it, even from the row a click focused", async ({ page, project, agent }) => {
  agent.script('Which optimizer should the baseline use?', [{ say: 'Start with AdamW.' }])
  await project.open(page)
  await send(page, 'Which optimizer should the baseline use?')
  await expect(page.getByText('Start with AdamW.')).toBeVisible()

  const row = page.locator('.session-row').first()
  const card = page.locator('[data-session-hover-card]')
  await row.click()
  await page.getByText('Start with AdamW.').hover()
  await row.hover()
  await expect(card).toBeVisible()

  const box = (await card.boundingBox())!
  const title = await card.getAttribute('aria-label')
  await test.step('a diagonal path over the rows below reaches the card', async () => {
    await page.mouse.move(box.x + 20, box.y + box.height - 10, { steps: 12 })
    await expect(card).toHaveAttribute('aria-label', title!)
  })
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2, { steps: 4 })
  await page.mouse.move(box.x + box.width + 120, box.y + box.height / 2, { steps: 4 })
  await expect(card).toBeHidden()
})

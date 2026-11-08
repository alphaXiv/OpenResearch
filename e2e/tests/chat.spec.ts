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

test('Stop ends the turn, drops the queue, and the chat takes the next message', async ({ page, project, agent }) => {
  agent.script('Train for ten epochs.', [{ say: 'Epoch 1 of 10. ' }, { hold: 'training' }, { say: 'Epoch 2 of 10.' }])
  await project.open(page)
  await send(page, 'Train for ten epochs.')
  await agent.reached('training')
  await send(page, 'Plot the loss when it finishes.')
  await expect(page.getByText('Queued', { exact: true })).toBeVisible()

  await page.getByRole('button', { name: 'Stop' }).click()
  await expect(page.getByRole('button', { name: 'Stop' })).toBeHidden()
  await expect(page.getByText('Queued', { exact: true })).toBeHidden()
  await expect(page.getByText('Epoch 1 of 10.')).toBeVisible()
  await expect(page.getByText('Plot the loss when it finishes.')).toBeHidden()

  agent.script('Train for two epochs instead.', [{ say: 'Trained for two epochs.' }])
  await send(page, 'Train for two epochs instead.')
  await expect(page.getByText('Trained for two epochs.')).toBeVisible()
  await expect(page.getByText('Epoch 2 of 10.')).toBeHidden()
})

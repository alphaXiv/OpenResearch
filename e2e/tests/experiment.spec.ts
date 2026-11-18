import { expect, send, test } from '../src/test.ts'

test('an agent runs its baseline on this machine, the run shows in Experiments, and its end wakes the chat', async ({ page, project, agent }) => {
  // What an agent following the orx-create and orx-git skills does in its worktree.
  agent.script('Run a baseline.', [
    {
      run: [
        'set -e',
        `id=$(orx create-experiment ${project.id} --title Baseline --run-command 'sh train.sh' | sed -n 's/^ *id: *//p')`,
        'git checkout -q orx/baseline',
        "printf 'echo loss=0.125\\n' > train.sh",
        "git add train.sh && git commit -qm 'Add the training script'",
        'orx exp run "$id" --backend local',
        'orx exp wake "$id"',
      ].join('\n'),
    },
    { say: 'Launched the baseline.' },
  ])
  agent.script('finished with status **done**', [{ say: 'The baseline finished at loss 0.125.' }])
  await project.open(page)
  await send(page, 'Run a baseline.')

  await test.step('the run ends and wakes the chat with its result', async () => {
    // The supervisor polls the job.
    await expect(page.getByText('The baseline finished at loss 0.125.')).toBeVisible({ timeout: 30_000 })
    await expect(page.getByRole('button', { name: /^Worked for/ })).toHaveAttribute('aria-expanded', 'false')
    await expect(page.getByText('Launched the baseline.', { exact: true })).toBeVisible()
  })

  await test.step('Experiments lists the baseline as done, with its log', async () => {
    await page.getByRole('button', { name: 'Experiments' }).click()
    const baseline = page.getByRole('list', { name: 'Experiments' }).getByRole('listitem').filter({ hasText: 'Baseline' })
    await expect(baseline).toContainText('Done')
    await expect(baseline).toContainText('1 run')
    await baseline.getByRole('button', { name: 'Logs' }).click()
    await expect(page.getByText('loss=0.125')).toBeVisible()
  })
})

test('Done offers to stop a chat’s experiment and its cancellation leaves the chat in Done', async ({ page, project, agent }) => {
  agent.script('Run the long baseline.', [
    {
      run: [
        'set -e',
        `id=$(orx create-experiment ${project.id} --title 'Long baseline' --run-command 'sleep 300' | sed -n 's/^ *id: *//p')`,
        'orx exp run "$id" --backend local',
      ].join('\n'),
    },
    { say: 'The long baseline is running.' },
  ])
  await project.open(page)
  await send(page, 'Run the long baseline.')
  const running = page.getByRole('button', { name: 'Stop the run' })
  // The local backend starts the job through orx-host.
  await expect(running).toBeVisible({ timeout: 30_000 })
  await expect(page.getByText('The long baseline is running.', { exact: true })).toBeVisible()

  const sidebar = page.getByRole('complementary')
  await sidebar.getByRole('button', { name: 'Filter sessions' }).click()
  await sidebar.getByRole('button', { name: 'Inbox', exact: true }).click()
  const chat = sidebar.getByRole('button', { name: /Run the long baseline.*E2E Agent/i })
  const done = chat.getByRole('button', { name: 'Mark as done' })
  const dialog = page.getByRole('dialog', { name: 'Stop and mark as done?' })
  await chat.hover()
  await done.click()
  await expect(dialog).toContainText('the experiments it started')
  await page.keyboard.press('Escape')
  await expect(dialog).toBeHidden()
  await expect(running).toBeVisible()

  await chat.hover()
  await done.click()
  await dialog.getByRole('button', { name: 'Stop and mark done' }).click()
  await expect(dialog).toBeHidden()

  await page.getByRole('button', { name: 'Experiments' }).click()
  const baseline = page.getByRole('list', { name: 'Experiments' }).getByRole('listitem').filter({ hasText: 'Long baseline' })
  await expect(baseline).toContainText('Cancelled', { timeout: 30_000 })
  const saved = await page.request.get(`/api/chat/sessions?projectId=${project.id}`)
  expect(await saved.json()).toMatchObject({ sessions: [{ archived: true }] })
})

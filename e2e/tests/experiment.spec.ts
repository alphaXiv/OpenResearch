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

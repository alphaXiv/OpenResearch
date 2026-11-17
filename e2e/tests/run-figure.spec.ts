import { send, test, expect } from '../src/test.ts'

test('an agent cites a completed run with its first block', async ({ page, project, agent, orx }) => {
  test.slow()
  agent.script('Launch the figure example.', [
    { run: [
      'set -e',
      `id=$(orx create-experiment ${project.id} --title 'Figure example' --run-command 'echo "::orx:: {\\"step\\":1,\\"of\\":1,\\"m\\":{\\"loss\\":0.125}}"' | sed -n 's/^ *id: *//p')`,
      'orx exp run "$id" --backend local',
      'orx exp wake "$id"',
    ].join('\n') },
    { say: 'Figure example launched.' },
  ])
  const hold = `figure-result-${project.id}`
  agent.script('finished with status **done**', [{ hold }, { say: 'The loss is ready.' }])
  await project.open(page)
  await send(page, 'Launch the figure example.')
  await agent.reached(hold)
  const shown = await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json()
  const run = shown.find((entry: { experimentName: string }) => entry.experimentName === 'Figure example').run.id
  agent.release(hold)
  await expect(page.getByText('The loss is ready.', { exact: true })).toBeVisible()
  agent.script('Cite that result.', [{ say: `Loss reached 0.125. <run id="${run}" label="Loss result" figure/>` }])
  await send(page, 'Cite that result.')
  const message = page.locator('.md').filter({ hasText: 'Loss reached 0.125.' })
  await expect(message.getByRole('img', { name: /loss/i })).toBeVisible()
  await expect(message.getByRole('button', { name: 'Loss result' })).toBeVisible()
  await message.getByRole('button', { name: 'Loss result' }).click()
  await expect(page.getByRole('tab', { name: /Figure example/ })).toBeVisible()
})

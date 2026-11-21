import { send, test, expect } from '../src/test.ts'

test('file citations open the artifact they name and never a missing file', async ({ page, project, agent, orx }) => {
  const listing = await page.request.get(`${orx.url}/api/projects/${project.id}/files`)
  await expect(listing).toBeOK()
  const { dir } = await listing.json()
  agent.script('Write the results.', [
    { run: [
      `mkdir -p '${dir}/exp1' '${dir}/.scratch'`,
      `printf 'kept\\n' > '${dir}/.scratch/notes.md'`,
      `printf '| lr | loss |\\n| --- | --- |\\n| 0.1 | 0.42 |\\n' > '${dir}/exp1/results_table.md'`,
      `printf '# Summary\\n\\nLoss fell to 0.42.\\n' > '${dir}/report.md'`,
    ].join(' && ') },
    { say: [
      'Results are in <file path="results_table.md" />, summarized in <file path="report.md" />.',
      'Hidden notes: <file path="artifacts/.scratch/notes.md" />.',
      'The drafts <file path="missing_table.md" /> and <file path="artifacts/missing_plot.png" /> were never written.',
    ].join(' ') },
  ])
  await project.open(page)
  await send(page, 'Write the results.')

  const message = page.locator('.md').filter({ hasText: 'Results are in' })
  const report = message.getByRole('button', { name: /report\.md/ })
  const table = message.getByRole('button', { name: /results_table\.md/, expanded: false })
  await expect(report).toHaveAttribute('title', /artifacts\/report\.md/)
  await expect(table).toBeVisible()

  await test.step('a missing file stays plain text', async () => {
    await expect(message).toContainText('missing_plot.png')
    await expect(message.getByRole('button', { name: /missing_table\.md/ })).toHaveCount(0)
    await expect(message.getByRole('button', { name: /missing_plot\.png/ })).toHaveCount(0)
  })
  await test.step('a top-level artifact opens directly', async () => {
    await report.click()
    await expect(page.getByText('Loss fell to 0.42.', { exact: true })).toBeVisible()
  })
  await test.step('a dotfile artifact the listing skips still opens', async () => {
    await expect(message.getByRole('button', { name: /notes\.md/ })).toBeVisible()
  })
  await test.step('a nested artifact is offered by its folder', async () => {
    await table.click()
    await page.getByRole('menuitem', { name: /artifacts\/exp1/ }).click()
    await expect(page.getByText('0.42', { exact: true })).toBeVisible()
  })
})

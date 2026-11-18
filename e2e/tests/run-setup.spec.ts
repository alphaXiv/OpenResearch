import { spawnSync } from 'node:child_process'
import { mkdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { expect, send, test } from '../src/test.ts'

const plain = (text: string) => text.replace(/[⁦-⁩]/g, '').replace(/\s+/g, ' ')
const report = 'const say = (record) => console.log("::orx:: " + JSON.stringify(record))'

test('the child view keeps training outside Setup before its first point', async ({ page, project, agent, orx }) => {
  test.slow()
  const dir = path.join(orx.root, 'stages')
  await mkdir(dir)
  for (const gate of ['label', 'train', 'point']) {
    expect(spawnSync('mkfifo', [path.join(dir, gate)]).status).toBe(0)
  }
  await writeFile(path.join(dir, 'view.xml'), `
<signal name="wer" label="Test WER" digits="3" better="lower"/>
<signal name="dev_loss" label="Dev loss" digits="3" better="lower"/>
<view headline="wer dev_loss">
  <chart y="dev_loss" phase="train"/>
  <stats of="wer" phase="eval"/>
</view>
`)
  await writeFile(path.join(dir, 'parent.mjs'), `
${report}
say({ plan: [{ phase: 'install', of: 1 }, { phase: 'download', of: 3, unit: 'files' }, { phase: 'eval', of: 2, unit: 'clips' }] })
say({ phase: 'install', step: 1, of: 1 })
say({ phase: 'download', step: 3, of: 3, unit: 'files' })
say({ phase: 'eval', step: 2, of: 2, unit: 'clips', m: { wer: 0.0512 } })
`)
  await writeFile(path.join(dir, 'child.mjs'), `
import { readFileSync } from 'node:fs'
${report}
const gate = (name) => readFileSync(new URL(name, import.meta.url))
say({ plan: [{ phase: 'install', of: 1 }, { phase: 'download', of: 3, unit: 'files' }, { phase: 'label', of: 5086, unit: 'clips' }, { phase: 'train', of: 272, unit: 'steps' }, { phase: 'eval', of: 2, unit: 'clips' }] })
say({ phase: 'install', step: 1, of: 1 })
say({ phase: 'download', step: 3, of: 3, unit: 'files' })
say({ phase: 'label', step: 4351, of: 5086, unit: 'clips' })
gate('label')
say({ phase: 'train', step: 3, of: 272, unit: 'steps' })
gate('train')
say({ step: 16, m: { dev_loss: 0.316 } })
gate('point')
say({ step: 272, m: { dev_loss: 0.156 } })
say({ phase: 'eval', step: 2, of: 2, unit: 'clips', m: { wer: 0.0355 } })
`)

  agent.script('Launch the stock run.', [
    { run: [
      'set -e',
      `id=$(orx create-experiment ${project.id} --title 'Stock model' --run-command '${process.execPath} ${dir}/parent.mjs' | sed -n 's/^ *id: *//p')`,
      `echo "$id" > ${dir}/parent-id`,
      'orx exp run "$id" --backend local',
    ].join('\n') },
    { say: 'Launched the stock run.' },
  ])
  agent.script('finished with status **done**', [{ say: 'Stock run finished.' }])
  await project.open(page)
  await send(page, 'Launch the stock run.')
  await expect(page.getByText('Stock run finished.', { exact: true })).toBeVisible({ timeout: 60_000 })

  agent.script('Launch the labelled run.', [
    { run: [
      'set -e',
      `id=$(orx create-experiment ${project.id} --parent "$(cat ${dir}/parent-id)" --title 'Labelled model' --run-command '${process.execPath} ${dir}/child.mjs' | sed -n 's/^ *id: *//p')`,
      `orx exp view set "$id" --file ${dir}/view.xml`,
      'orx exp run "$id" --backend local',
    ].join('\n') },
    { say: 'Launched the labelled run.' },
  ])
  agent.script('finished with status **done**', [{ say: 'Labelled run finished.' }])
  await send(page, 'Launch the labelled run.')

  const banner = page.getByRole('region', { name: 'Runs in this chat' })
  const status = banner.getByRole('button', { name: /Labelled model/ })
  const card = page.getByRole('dialog', { name: /Labelled model/ })
  const stages = card.getByRole('listitem')
  const check = async (words: RegExp, active: string) => {
    await expect.poll(async () => plain(await status.innerText()), { timeout: 60_000 }).toMatch(words)
    await page.mouse.move(5, 5)
    await expect(card).toBeHidden()
    await status.hover()
    await expect(stages).toHaveCount(3)
    await expect(stages.nth(0)).not.toContainText('train')
    await expect(stages.nth(1)).toContainText('train')
    await expect(stages.nth(2)).toContainText('eval')
    await expect(card.locator('[aria-current="step"]')).toContainText(active)
  }
  const release = (gate: string) => writeFile(path.join(dir, gate), 'go\n')
  try {
    await check(/Setup \(1\/3\).*4,351/, 'Setup')
    await release('label')
    await check(/Setup \(1\/3\).*3 of 272/, 'Setup')
    await release('train')
    await check(/train \(2\/3\).*16 of 272/, 'train')
    await release('point')
    await expect(page.getByText('Labelled run finished.', { exact: true })).toBeVisible({ timeout: 60_000 })
  } finally {
    const { runs } = await (await page.request.get(`${orx.url}/api/projects/${project.id}/runs`)).json()
    for (const run of runs.filter((run: { status: string }) => run.status === 'running')) {
      await page.request.post(`${orx.url}/api/runs/${run.id}/cancel`, { headers: { origin: orx.url } })
      await expect.poll(async () => {
        const { run: current } = await (await page.request.get(`${orx.url}/api/runs/${run.id}`)).json()
        return current.status
      }).toMatch(/^(done|failed|cancelled)$/)
    }
  }
})

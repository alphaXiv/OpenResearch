import { spawnSync } from 'node:child_process'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { writeFile } from 'node:fs/promises'
import path from 'node:path'
import { expect as baseExpect, send, test } from '../src/test.ts'

// Waits on the run or the agent, which a busy machine slows, get this budget.
const expect = baseExpect.configure({ timeout: 30_000 })

const VIEW = `<signal name="loss" label="Training loss" digits="3" better="lower" />
<view headline="loss">
  <chart y="loss" ghost="parent" />
</view>
`

test("an agent's view labels its runs, and a child's line says how it compares with the parent", async ({ page, project, agent, orx }) => {
  test.slow()
  const dir = path.join(orx.root, `view-${project.id}`)
  mkdirSync(dir)
  writeFileSync(path.join(dir, 'view.xml'), VIEW)
  writeFileSync(path.join(dir, 'typo.xml'), VIEW.replace('y="loss"', 'y="los"'))
  expect(spawnSync('mkfifo', [path.join(dir, 'half')]).status).toBe(0)
  // The loss is argv[2] times 2/step; the child stops half way until the test opens its gate.
  const script = path.join(dir, 'train.mjs')
  writeFileSync(script, `import { readFileSync } from 'node:fs'
const scale = Number(process.argv[2])
const tick = () => new Promise((resolve) => setTimeout(resolve, 100))
const started = performance.now()
process.stdout.write('::orx:: {"step":0,"of":20,"t":0}\\n')
for (let step = 1; step <= 20; step++) {
  await tick()
  process.stdout.write('::orx:: {"step":' + step + ',"t":' + ((performance.now() - started) / 1000) + ',"m":{"loss":' + (scale * 2 / step).toFixed(4) + '}}\\n')
  if (scale < 1 && step === 10) readFileSync(${JSON.stringify(path.join(dir, 'half'))})
}
`)
  const command = (scale: number) => `${process.execPath} ${script} ${scale}`
  const banner = page.getByRole('region', { name: 'Runs in this chat' })

  await test.step('the baseline is drawn with the view its agent set', async () => {
    agent.script('Set the view and launch Base.', [
      { run: [
        'set -e',
        `id=$(orx create-experiment ${project.id} --title Base --run-command '${command(1)}' | sed -n 's/^ *id: *//p')`,
        `echo "$id" > ${dir}/base`,
        `orx exp view set "$id" --file ${dir}/view.xml`,
        'orx exp run "$id" --backend local',
        'orx exp wake "$id"',
      ].join('\n') },
      { say: 'Launched Base.' },
    ])
    agent.script('finished with status **done**', [{ say: 'Base finished.' }])
    await project.open(page)
    await send(page, 'Set the view and launch Base.')
    await expect(page.getByText('Base finished.', { exact: true })).toBeVisible()
    await expect(banner).toContainText('Training loss')
  })

  await test.step("the child's line says how far it is ahead of its parent", async () => {
    agent.script('Launch Child.', [
      { run: [
        'set -e',
        `id=$(orx create-experiment ${project.id} --title Child --parent "$(cat ${dir}/base)" --run-command '${command(0.8)}' | sed -n 's/^ *id: *//p')`,
        'orx exp run "$id" --backend local',
        'orx exp wake "$id"',
      ].join('\n') },
      { say: 'Launched Child.' },
    ])
    await send(page, 'Launch Child.')
    const child = banner.filter({ hasText: 'Child' })
    await expect(child).toContainText(/Training loss/)
    // At step 10 the child's loss is 0.160 against the parent's 0.200.
    await expect(child).toContainText(/0\.040⁩? better than parent/)
  })

  await test.step('a misspelt signal is refused with the names the run reported', async () => {
    const runs = await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json()
    const run = runs.find((shown: { experimentName: string }) => shown.experimentName === 'Child').run.id
    agent.script('Check the view.', [
      { run: `orx exp view check ${run} --file ${dir}/typo.xml > ${dir}/check.txt 2>&1 || true` },
      { say: 'Checked.' },
    ])
    await send(page, 'Check the view.')
    await expect(page.getByText('Checked.', { exact: true })).toBeVisible()
    expect(readFileSync(path.join(dir, 'check.txt'), 'utf8')).toContain(
      `typo.xml:3:3: blocks[0].chart[0]: no signal "los"; seen: loss. Run \`orx exp view check ${run} --file ${dir}/typo.xml\`.`,
    )
    agent.script('finished with status **done**', [{ say: 'Child finished.' }])
    await writeFile(path.join(dir, 'half'), 'go\n')
    await expect(page.getByText('Child finished.', { exact: true })).toBeVisible()
  })
})

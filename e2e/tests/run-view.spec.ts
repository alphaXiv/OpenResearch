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
    // The line leaves once the agent says what came of the run, so it is read before that.
    const hold = `base-result-${project.id}`
    agent.script('finished with status **done**', [{ hold }, { say: 'Base finished.' }])
    await project.open(page)
    await send(page, 'Set the view and launch Base.')
    await agent.reached(hold)
    await expect(banner).toContainText('Training loss')
    agent.release(hold)
    await expect(page.getByText('Base finished.', { exact: true })).toBeVisible()
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

test("an agent changes a running run's view and patterns, and the open strip and page follow without a reload", async ({ page, project, agent, orx }, info) => {
  test.slow()
  const dir = path.join(orx.root, `change-${project.id}`)
  mkdirSync(dir)
  writeFileSync(path.join(dir, 'view.xml'), VIEW)
  writeFileSync(path.join(dir, 'eval.xml'), `<signal name="acc" label="Eval accuracy" digits="2" better="higher" />
<signal name="loss" label="Training loss" digits="3" />
<view headline="acc">
  <chart y="acc" />
  <chart y="loss" />
</view>
`)
  // The log has said this all along; only a pattern reads it.
  writeFileSync(path.join(dir, 'patterns.json'), JSON.stringify([{ regex: '^eval acc (?P<acc>[\\d.]+)$' }]))
  expect(spawnSync('mkfifo', [path.join(dir, 'later')]).status).toBe(0)
  const script = path.join(dir, 'train.mjs')
  writeFileSync(script, `import { readFileSync } from 'node:fs'
const tick = () => new Promise((resolve) => setTimeout(resolve, 100))
const started = performance.now()
process.stdout.write('::orx:: {"step":0,"of":20,"t":0}\\n')
for (let step = 1; step <= 20; step++) {
  await tick()
  process.stdout.write('::orx:: {"step":' + step + ',"t":' + ((performance.now() - started) / 1000) + ',"m":{"loss":' + (2 / step).toFixed(4) + '}}\\n')
  process.stdout.write('eval acc ' + (0.5 + step / 100).toFixed(2) + '\\n')
  if (step === 8) readFileSync(${JSON.stringify(path.join(dir, 'later'))})
}
`)
  const now = `${process.execPath} -e 'process.stdout.write(String(Date.now()))'`
  await page.request.post(`${orx.url}/api/settings/ui-state`, { headers: { origin: orx.url }, data: { runBanner: 'strip' } })
  const banner = page.getByRole('region', { name: 'Runs in this chat' })
  const latest = page.locator('section', { has: page.getByRole('heading', { name: 'Latest run', exact: true }) })

  await test.step('the run is drawn with the view it was launched with, on the strip and its page', async () => {
    agent.script('Launch Eval.', [
      { run: [
        'set -e',
        `id=$(orx create-experiment ${project.id} --title Eval --run-command '${process.execPath} ${script}' | sed -n 's/^ *id: *//p')`,
        `echo "$id" > ${dir}/id`,
        `orx exp view set "$id" --file ${dir}/view.xml`,
        'orx exp run "$id" --backend local',
        'orx exp wake "$id"',
      ].join('\n') },
      { say: 'Launched Eval.' },
    ])
    await project.open(page)
    await send(page, 'Launch Eval.')
    await expect(banner).toContainText(/Step [⁦⁨]?8[⁩]? of [⁦⁨]?20[⁩]?/)
    await expect(banner).toContainText('Training loss')
    await banner.getByRole('button', { name: /^\W?eval\W?$/i }).click()
    await expect(latest.getByRole('heading', { name: /^\W?Training loss\W?$/ })).toBeVisible()
    await info.attach('before', { body: await page.screenshot(), contentType: 'image/png' })
  })

  await test.step('mid-run, a new view and patterns redraw both, reading what the log already said', async () => {
    // Each surface records the moment it first shows the change, so the delay is measured, not polled.
    await page.evaluate(() => {
      const seen: Record<string, number> = {}
      Object.assign(window, { seen })
      const banner = () => document.querySelector('[aria-label="Runs in this chat"]')
      const marks: Record<string, () => boolean> = {
        strip: () => !!banner()?.textContent?.includes('Eval accuracy'),
        value: () => !!banner()?.textContent?.replace(/[⁦-⁩]/g, '').includes('0.58'),
        page: () => [...document.querySelectorAll('h3')].some((heading) => heading.textContent?.replace(/[⁦-⁩]/g, '') === 'Eval accuracy' && !banner()?.contains(heading)),
      }
      const look = setInterval(() => {
        for (const [name, shown] of Object.entries(marks)) if (!(name in seen) && shown()) seen[name] = Date.now()
        if (Object.keys(seen).length === Object.keys(marks).length) clearInterval(look)
      }, 20)
    })
    agent.script('Show the eval accuracy instead.', [
      { run: `orx exp view set "$(cat ${dir}/id)" --file ${dir}/eval.xml --pattern ${dir}/patterns.json && ${now} > ${dir}/set-at` },
      { say: 'Now showing the eval accuracy.' },
    ])
    await send(page, 'Show the eval accuracy instead.')
    await expect(banner).toContainText('Eval accuracy')
    await expect(banner).toContainText(/0\.58/)
    await expect(latest.getByRole('heading', { name: /^\W?Eval accuracy\W?$/ })).toBeVisible()
    await expect(latest.getByRole('heading', { name: /^\W?Training loss\W?$/ })).toBeVisible()

    const runId = (await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json())
      .find((shown: { experimentName: string }) => shown.experimentName === 'Eval').run.id
    const monitor = await (await page.request.get(`${orx.url}/api/runs/${runId}/monitor`)).json()
    expect(monitor.series.acc.map(([x]: [number]) => x), 'every step the log printed before the pattern existed').toEqual([1, 2, 3, 4, 5, 6, 7, 8])

    const setAt = Number(readFileSync(path.join(dir, 'set-at'), 'utf8'))
    const seen = await page.evaluate(() => (window as unknown as { seen: Record<string, number> }).seen)
    const delays = Object.fromEntries(Object.entries(seen).map(([name, at]) => [name, at - setAt]))
    info.annotations.push({ type: 'shown after the view was set (ms)', description: JSON.stringify(delays) })
    await info.attach('after', { body: await page.screenshot(), contentType: 'image/png' })
  })

  agent.script('finished with status **done**', [{ say: 'Eval finished.' }])
  await writeFile(path.join(dir, 'later'), 'go\n')
  await expect(page.getByText('Eval finished.', { exact: true })).toBeVisible()
})

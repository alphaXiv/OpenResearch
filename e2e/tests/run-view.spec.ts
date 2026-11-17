import { spawnSync } from 'node:child_process'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { writeFile } from 'node:fs/promises'
import path from 'node:path'
import { expect as baseExpect, send, test } from '../src/test.ts'

// Waits on the run or the agent, which a busy machine slows, get this budget.
const expect = baseExpect.configure({ timeout: 30_000 })

const VIEW = `<signal name="loss" label="Training loss" digits="3" better="lower" />
<view headline="loss">
  <chart y="loss" phase="train" />
</view>
`

test("an agent's view labels its runs, and a child's line says how it compares with the parent", async ({ page, project, agent, orx }) => {
  test.slow()
  const dir = path.join(orx.root, `view-${project.id}`)
  mkdirSync(dir)
  writeFileSync(path.join(dir, 'view.xml'), VIEW)
  writeFileSync(path.join(dir, 'off.xml'), VIEW.replace('<chart ', '<chart ghost="false" '))
  writeFileSync(path.join(dir, 'typo.xml'), VIEW.replace('y="loss"', 'y="los"'))
  expect(spawnSync('mkfifo', [path.join(dir, 'half')]).status).toBe(0)
  // The loss is argv[2] times 2/step; the child stops half way until the test opens its gate.
  const script = path.join(dir, 'train.mjs')
  writeFileSync(script, `import { readFileSync } from 'node:fs'
const scale = Number(process.argv[2])
const tick = () => new Promise((resolve) => setTimeout(resolve, 100))
const started = performance.now()
process.stdout.write('::orx:: {"phase":"train","step":0,"of":20,"t":0}\\n')
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
    const child = banner.getByRole('group', { name: 'Child', exact: true })
    await expect(child).toContainText(/Training loss/, { timeout: 90_000 })
    await expect(child.locator('svg path[stroke-dasharray]')).toHaveCount(1)
    const shots = path.resolve(import.meta.dirname, '../../.shots/k1')
    mkdirSync(shots, { recursive: true })
    await page.getByRole('button', { name: 'Close panel', exact: true }).click()
    for (const width of [1440, 420]) {
      await page.setViewportSize({ width, height: 1000 })
      for (const colorScheme of ['light', 'dark'] as const) {
        await page.emulateMedia({ colorScheme, reducedMotion: 'reduce' })
        await expect(page.locator('html')).toHaveAttribute('data-theme', colorScheme)
        await page.screenshot({ path: path.join(shots, `parent-${colorScheme}-${width}.png`) })
      }
    }
    const shown = await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json()
    const childId = shown.find((entry: { experimentName: string }) => entry.experimentName === 'Child').run.experimentId
    agent.script('Hide the parent curve.', [
      { run: `orx exp view set ${childId} --file ${dir}/off.xml` },
      { say: 'Parent curve hidden.' },
    ])
    await send(page, 'Hide the parent curve.')
    await expect(child.locator('svg path[stroke-dasharray]')).toHaveCount(0)
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
  <stats of="acc loss" />
</view>
`)
  // The log has said this all along; only a pattern reads it.
  writeFileSync(path.join(dir, 'patterns.json'), JSON.stringify([{ regex: '^eval acc (?P<acc>[\\d.]+)$' }]))
  expect(spawnSync('mkfifo', [path.join(dir, 'later')]).status).toBe(0)
  const script = path.join(dir, 'train.mjs')
  writeFileSync(script, `import { readFileSync } from 'node:fs'
const tick = () => new Promise((resolve) => setTimeout(resolve, 100))
const started = performance.now()
process.stdout.write('::orx:: {"phase":"train","step":0,"of":20,"t":0}\\n')
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
    await banner.getByRole('button', { name: /Eval/ }).hover()
    await page.getByRole('dialog').getByRole('link', { name: /Eval/ }).click()
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
        page: () => Array.from(document.querySelectorAll('h3')).some((heading) => heading.textContent?.replace(/[⁦-⁩]/g, '') === 'Eval accuracy' && !banner()?.contains(heading)),
      }
      const look = setInterval(() => {
        for (const [name, shown] of Object.entries(marks)) if (!(name in seen) && shown()) seen[name] = Date.now()
        if (Object.keys(seen).length === Object.keys(marks).length) clearInterval(look)
      }, 20)
    })
    const runId = (await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json())
      .find((shown: { experimentName: string }) => shown.experimentName === 'Eval').run.id
    agent.script('Show the eval accuracy instead.', [
      { run: [
        'set -e',
        `orx exp view show > ${dir}/before.txt`,
        `orx exp view set "$(cat ${dir}/id)" --file ${dir}/eval.xml --pattern ${dir}/patterns.json`,
        `${now} > ${dir}/set-at`,
      ].join('\n') },
      { say: 'Now showing the eval accuracy.' },
    ])
    await send(page, 'Show the eval accuracy instead.')
    await expect(banner).toContainText('Eval accuracy')
    await expect(banner).toContainText(/0\.58/)
    await expect(latest.getByRole('heading', { name: /^\W?Eval accuracy\W?$/ })).toBeVisible()
    await expect(latest.getByRole('heading', { name: /^\W?Training loss\W?$/ })).toBeVisible()
    await expect(page.getByText('Now showing the eval accuracy.', { exact: true })).toBeVisible()
    agent.script('Describe the updated card.', [
      { run: `orx exp view show ${runId} > ${dir}/after.txt` },
      { say: 'The card and its collected view now show eval accuracy.' },
    ])
    await send(page, 'Describe the updated card.')
    await expect(page.getByText('The card and its collected view now show eval accuracy.', { exact: true })).toBeVisible()
    const before = readFileSync(path.join(dir, 'before.txt'), 'utf8')
    const after = readFileSync(path.join(dir, 'after.txt'), 'utf8')
    expect(before).toContain('y=loss; x=step; observed x=1 .. 8; total=20')
    expect(before).not.toContain('y=acc')
    expect(after).toContain('y=acc; x=step; observed x=1 .. 8; total=20')
    expect(after).toContain('finite retained points=8')
    const [compact, runPage] = after.split('On the run page')
    expect(compact).toContain('Above the chat')
    expect(compact).toContain('Card headline: Eval accuracy (acc) = 0.58')
    expect(compact).toContain('Card chart: Eval accuracy (acc)')
    expect(compact).toContain('8 points: step 1 = 0.51')
    expect(compact).not.toContain('Stats:')
    expect(runPage).toContain('Stats: Eval accuracy')
    const shots = path.resolve(import.meta.dirname, '../../.shots/k2')
    mkdirSync(shots, { recursive: true })
    writeFileSync(path.join(shots, 'view-show.txt'), after)
    for (const width of [1440, 420]) {
      await page.setViewportSize({ width, height: 1000 })
      if (width === 420) {
        await page.getByTitle(/^Close tab/).click()
        await expect(banner).toBeVisible()
      }
      for (const colorScheme of ['light', 'dark'] as const) {
        await page.emulateMedia({ colorScheme })
        await expect(page.locator('html')).toHaveAttribute('data-theme', colorScheme)
        await expect.poll(() => banner.evaluate((root) => root.getAnimations({ subtree: true }).filter((a) => a instanceof CSSTransition).length)).toBe(0)
        await page.screenshot({ path: path.join(shots, `card-${colorScheme}-${width}.png`) })
      }
    }
    expect(after).toContain('Stats: Eval accuracy (acc) = 0.58; Training loss (loss) = 0.250')
    expect(after).toContain('8 matches')
    await info.attach('view-show-before', { body: before, contentType: 'text/plain' })
    await info.attach('view-show-after', { body: after, contentType: 'text/plain' })
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

test('a missing run report wakes the agent once and its live view can be repaired', async ({ page, project, agent, orx }) => {
  test.slow()
  const dir = path.join(orx.root, `instrumentation-${project.id}`)
  mkdirSync(dir)
  const gate = path.join(dir, 'finish')
  expect(spawnSync('mkfifo', [gate]).status).toBe(0)
  const script = path.join(dir, 'train.mjs')
  writeFileSync(script, `import { readFileSync } from 'node:fs'
process.stdout.write('::orx:: {"phase":"train","step":1,"m":{"loss":2}}\\n')
process.stdout.write('::orx:: {"step":2,"m":{"loss":1}}\\n')
readFileSync(${JSON.stringify(gate)})
process.stdout.write('::orx:: {"step":3,"m":{"loss":0.5}}\\n')
`)
  writeFileSync(path.join(dir, 'broken.xml'), '<signal name="missing"/><view headline="missing"><chart y="missing"/></view>')
  writeFileSync(path.join(dir, 'view.xml'), VIEW)
  writeFileSync(path.join(dir, 'patterns.json'), JSON.stringify([{ regex: '^never (?P<missing>\\d+)$' }]))
  writeFileSync(path.join(dir, 'clear.json'), '[]')
  writeFileSync(path.join(dir, 'plan.json'), JSON.stringify({ plan: [{ phase: 'train', of: 3, unit: 'steps' }] }))
  const warning = `instrumentation-${project.id}`
  agent.script('Launch the reporting smoke test.', [
    { run: [
      'set -e',
      `id=$(orx create-experiment ${project.id} --title Reports --run-command '${process.execPath} ${script}' | sed -n 's/^ *id: *//p')`,
      `orx exp view set "$id" --file ${dir}/broken.xml --pattern ${dir}/patterns.json`,
      'orx exp run "$id" --backend local',
      'orx exp wake "$id"',
    ].join('\n') },
    { say: 'Reporting smoke test launched.' },
  ])
  agent.script('needs an instrumentation check', [
    { hold: warning },
    { run: [
      'set -e',
      `read run exp < ${dir}/ids`,
      `orx exp view check "$run" --json > ${dir}/check.json`,
      `orx exp view show "$run" > ${dir}/before.txt`,
      `orx exp view set "$exp" --file ${dir}/view.xml --pattern ${dir}/clear.json --config ${dir}/plan.json`,
      `orx exp view show "$run" > ${dir}/after.txt`,
    ].join('\n') },
    { say: 'Progress and chart corrected.' },
  ])
  agent.script('finished with status **done**', [{ say: 'Reporting smoke test finished.' }])
  await project.open(page)
  await send(page, 'Launch the reporting smoke test.')
  await agent.reached(warning)
  const shown = await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json()
  const run = shown.find((entry: { experimentName: string }) => entry.experimentName === 'Reports').run
  expect(run.status).toBe('running')
  writeFileSync(path.join(dir, 'ids'), `${run.id} ${run.experimentId}\n`)
  agent.release(warning)
  await expect(page.getByText('Progress and chart corrected.', { exact: true })).toBeVisible()
  const checked = JSON.parse(readFileSync(path.join(dir, 'check.json'), 'utf8'))
  expect(checked.patterns[0].matched).toBe(0)
  expect(checked.pending).toContain('missing')
  expect(checked.stages[0].of).toBeNull()
  const before = readFileSync(path.join(dir, 'before.txt'), 'utf8')
  const after = readFileSync(path.join(dir, 'after.txt'), 'utf8')
  expect(before).toContain('names not observed in collected data: missing')
  expect(before).toContain('never matched in the checked log')
  expect(after).toContain('done=2; of=3')
  expect(after).toContain('names not observed in collected data: none')
  const banner = page.getByRole('region', { name: 'Runs in this chat' })
  await expect(banner).toContainText('Training loss')
  await expect(banner).toContainText(/step [⁦⁨]?2[⁩]? of [⁦⁨]?3[⁩]?/i)
  await writeFile(gate, 'go\n')
  await expect(page.getByText('Reporting smoke test finished.', { exact: true })).toBeVisible()
})

test('a timed floor names the untimed stage above the chat', async ({ page, project, agent, orx }) => {
  test.slow()
  const dir = path.join(orx.root, `floor-${project.id}`)
  mkdirSync(dir)
  const gate = path.join(dir, 'finish')
  expect(spawnSync('mkfifo', [gate]).status).toBe(0)
  const script = path.join(dir, 'train.mjs')
  writeFileSync(script, `import { readFileSync } from 'node:fs'
process.stdout.write('::orx:: {"plan":[{"phase":"train","of":1000},{"phase":"sweep"}]}\\n')
for (let step = 0; step <= 100; step += 10) {
  process.stdout.write('::orx:: ' + JSON.stringify({phase:'train', step, of:1000, t:step*4.3, m:{loss:2-step/100}}) + '\\n')
}
readFileSync(${JSON.stringify(gate)})
`)
  agent.script('Launch timed floor.', [
    { run: [
      'set -e',
      `id=$(orx create-experiment ${project.id} --title 'Timed floor' --run-command '${process.execPath} ${script}' | sed -n 's/^ *id: *//p')`,
      'orx exp run "$id" --backend local',
      'orx exp wake "$id"',
    ].join('\n') },
    { say: 'Timed floor launched.' },
  ])
  const hold = `floor-result-${project.id}`
  agent.script('finished with status **done**', [{ hold }, { say: 'Timed floor finished.' }])
  await project.open(page)
  await send(page, 'Launch timed floor.')
  const banner = page.getByRole('region', { name: 'Runs in this chat' })
  await expect(banner).toContainText(/At least .* left/)
  await expect(banner.getByRole('img', { name: /^loss / })).toBeVisible()
  const shots = path.resolve(import.meta.dirname, '../../.shots/k1')
  mkdirSync(shots, { recursive: true })
  await page.getByRole('button', { name: 'Close panel', exact: true }).click()
  for (const width of [1440, 420]) {
    await page.setViewportSize({ width, height: 1000 })
    for (const colorScheme of ['light', 'dark'] as const) {
      await page.emulateMedia({ colorScheme, reducedMotion: 'reduce' })
      await expect(page.locator('html')).toHaveAttribute('data-theme', colorScheme)
      await page.screenshot({ path: path.join(shots, `floor-${colorScheme}-${width}.png`) })
      await banner.getByRole('button', { name: /Timed floor/, exact: false }).first().click()
      const card = page.getByRole('dialog')
      await expect(card).toContainText(/Whole run: At least .* left.*sweep.*not timed/)
      await page.screenshot({ path: path.join(shots, `floor-card-${colorScheme}-${width}.png`) })
      await page.keyboard.press('Escape')
      await expect(card).not.toBeVisible()
    }
  }
  await writeFile(gate, 'go\n')
  await agent.reached(hold)
  agent.release(hold)
  await expect(page.getByText('Timed floor finished.', { exact: true })).toBeVisible()
})

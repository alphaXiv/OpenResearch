import { spawnSync } from 'node:child_process'
import { writeFile } from 'node:fs/promises'
import { mkdirSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import type { Locator, Page, TestInfo } from '@playwright/test'
import type { Agent } from '../src/agent.ts'
import type { Orx, Project } from '../src/orx.ts'
import { expect as baseExpect, send, test } from '../src/test.ts'

// Waits on the run or the agent, which a busy machine slows, get this budget; the restart's and the clock's set their own.
const expect = baseExpect.configure({ timeout: 30_000 })

/** A payload that reports on its own clock and stops at each of `gates` until the test opens it. */
function payload(orx: Orx, name: string, gates: string[], body: string) {
  const dir = path.join(orx.root, name)
  mkdirSync(dir)
  for (const gate of gates) expect(spawnSync('mkfifo', [path.join(dir, gate)]).status).toBe(0)
  const script = path.join(dir, 'payload.mjs')
  writeFileSync(script, [
    "import { readFileSync } from 'node:fs'",
    `const gate = (name) => readFileSync(${JSON.stringify(dir)} + '/' + name)`,
    'const tick = () => new Promise((resolve) => setTimeout(resolve, 150))',
    body,
  ].join('\n'))
  return {
    command: `${process.execPath} ${script}`,
    open: (gate: string) => writeFile(path.join(dir, gate), 'go\n'),
  }
}

async function launch(page: Page, agent: Agent, project: Project, title: string, command: string) {
  agent.script(`Launch ${title}.`, [
    { run: [
      'set -e',
      `id=$(orx create-experiment ${project.id} --title ${title} --run-command '${command}' | sed -n 's/^ *id: *//p')`,
      'orx exp run "$id" --backend local',
      'orx exp wake "$id"',
    ].join('\n') },
    { say: `Launched ${title}.` },
  ])
  await project.open(page)
  await send(page, `Launch ${title}.`)
  const banner = page.getByRole('region', { name: 'Runs in this chat' })
  await expect(banner.getByRole('button', { name: 'Stop the run', exact: true })).toBeVisible()
  return banner
}

/**
 * Text and marks in `region` against what is really behind them, from resolved styles: text 4.5:1;
 * the bar's fill, the run's mark, lines, squares and the fills of bars 3:1. Text drawn but hidden from
 * assistive technology, such as a chart's legend, counts too. Returns each pair that falls short.
 */
async function contrast(region: Locator): Promise<string[]> {
  return region.evaluate((root) => {
    /** The ratio of two sRGB colors as WCAG defines it. */
    const ratio = (a: number[], b: number[]) => {
      const lum = (rgb: number[]) => rgb.map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4 }).reduce((s, v, i) => s + v * [0.2126, 0.7152, 0.0722][i], 0)
      const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p)
      return (x + 0.05) / (y + 0.05)
    }
    const canvas = document.createElement('canvas').getContext('2d')!
    const rgba = (color: string) => {
      canvas.clearRect(0, 0, 1, 1)
      canvas.fillStyle = '#000'
      canvas.fillStyle = color
      canvas.fillRect(0, 0, 1, 1)
      return Array.from(canvas.getImageData(0, 0, 1, 1).data)
    }
    const over = (top: number[], bottom: number[]) => top.slice(0, 3).map((v, i) => v * (top[3] / 255) + bottom[i] * (1 - top[3] / 255))
    const behind = (element: Element | null): number[] => {
      if (!element) return rgba(getComputedStyle(document.body).backgroundColor).slice(0, 3)
      const own = rgba(getComputedStyle(element).backgroundColor)
      const below = behind(element.parentElement)
      return own[3] === 0 ? below : over(own, below)
    }
    const failures: string[] = []
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const element = node.parentElement!
      const text = node.textContent!.replace(/[⁦-⁩]/g, '').trim()
      if (!text || element.closest('[hidden]') || element.getClientRects().length === 0) continue
      const style = getComputedStyle(element)
      const ink = over(rgba(style.color).map((v, i) => i === 3 ? v * Number(style.opacity) : v), behind(element))
      const value = ratio(ink, behind(element))
      if (value < 4.5) failures.push(`text "${text}" ${value.toFixed(2)}:1`)
    }
    // A tooltip's trigger is an img too, around text; a mark is drawn alone.
    const marks = Array.from(root.querySelectorAll('[role="progressbar"] i, span[role="img"]:empty, svg path, svg rect, span[aria-hidden] > i'))
    for (const mark of marks) {
      const style = getComputedStyle(mark)
      const paint = mark instanceof SVGRectElement ? (style.fill === 'none' ? style.stroke : style.fill) : mark instanceof SVGElement ? style.stroke : style.backgroundColor
      if (!paint || paint === 'none' || mark.getClientRects().length === 0) continue
      const track = mark.closest('[role="progressbar"]') ? behind(mark.parentElement!.parentElement) : behind(mark.parentElement)
      const value = ratio(over(rgba(paint), track), track)
      if (value < 3) failures.push(`mark ${mark.tagName.toLowerCase()} ${paint} ${value.toFixed(2)}:1`)
    }
    return failures
  })
}

async function themes(page: Page, banner: Locator, info: TestInfo, name: string) {
  for (const scheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: scheme })
    await expect(page.locator('html')).toHaveAttribute('data-theme', scheme)
    // Colors ease between themes; measure where they settle.
    await expect.poll(() => banner.evaluate((root) => root.getAnimations({ subtree: true }).filter((a) => a instanceof CSSTransition).length)).toBe(0)
    expect(await contrast(banner), `${name} in ${scheme}`).toEqual([])
    await info.attach(`${name}-${scheme}`, { body: await banner.screenshot(), contentType: 'image/png' })
  }
  await page.emulateMedia({ colorScheme: 'light' })
}

/** The Latest run section of the experiment page in the side pane. */
const latestRun = (page: Page) => page.locator('section', { has: page.getByRole('heading', { name: 'Latest run', exact: true }) })

/** Text as the page writes it, numbers wrapped in direction isolates. */
const said = (text: string) => new RegExp(text.replace(/\d+/g, (n) => `[⁦⁨]?${n}[⁩]?`))

const steps = async (page: Page, orx: Orx, runId: string) => {
  const monitor = await (await page.request.get(`${orx.url}/api/runs/${runId}/monitor`)).json()
  return { monitor, x: (monitor.series.loss ?? []).map(([x]: [number]) => x) as number[] }
}

test('a declared run shows its step, a growing bar, time left and its curve, through a restart to done', async ({ page, agent, launchOrx }, info) => {
  // Two theme passes and the run itself outlast the default budget on a busy machine.
  test.slow()
  const orx = await launchOrx()
  await orx.onboard()
  const project = await orx.createProject('monitored')
  const run = payload(orx, 'declared', ['nan', 'half', 'late'], `
const total = 40
const started = performance.now()
const report = (step, loss) => process.stdout.write('::orx:: {"step":' + step + ',"t":' + ((performance.now() - started) / 1000) + (loss === undefined ? '' : ',"m":{"loss":' + loss + '}') + '}\\n')
process.stdout.write('::orx:: {"step":0,"of":40,"unit":"steps","t":0}\\n')
for (let step = 1; step <= total; step++) {
  await tick()
  report(step, step === 10 ? 'NaN' : (2 / step).toFixed(4))
  if (step === 10) gate('nan')
  if (step === 20) gate('half')
  if (step === 30) gate('late')
}`)
  const hold = `result-${project.id}`
  agent.script('finished with status **done**', [{ hold }, { say: 'Training finished.' }])
  const banner = await launch(page, agent, project, 'Declared', run.command)
  const chart = banner.getByRole('img', { name: /^loss / })

  await test.step('a NaN reads NaN', async () => {
    await expect(banner).toContainText(said('Step 10 of 40'))
    await expect(chart).toHaveAccessibleName('loss NaN')
    await expect(banner).toContainText('NaN')
  })

  await test.step('half way the bar is half full, with time left and the latest value in the chart', async () => {
    await run.open('nan')
    await expect(banner).toContainText(said('Step 20 of 40'))
    await expect(banner.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '50')
    await expect(banner).toContainText(/\d+s⁩? left/)
    await expect(banner).not.toContainText('At least')
    await expect(chart).toHaveAccessibleName(said('loss 0.1'))
    await themes(page, banner, info, 'strip-running')
  })

  const runId = (await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json())[0].run.id
  await test.step('the run goes on while orx up is down; the open page catches up with each step once', async () => {
    await orx.restart('SIGTERM', () => run.open('half'))
    await expect(banner).toContainText(said('Step 30 of 40'), { timeout: 20_000 })
    await expect(banner.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '75')
    await expect(chart).toHaveAccessibleName(said('loss 0.0667'))
    const { x } = await steps(page, orx, runId)
    expect(x).toEqual(Array.from({ length: 30 }, (_, i) => i + 1))
    await run.open('late')
  })

  await test.step('the run ends and the line reads done', async () => {
    await agent.reached(hold)
    await expect(banner).toContainText('Done in')
    await expect(banner.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '100')
    const { monitor, x } = await steps(page, orx, runId)
    expect(x).toEqual(Array.from({ length: 40 }, (_, i) => i + 1))
    expect(monitor.series.loss[9]).toEqual([10, null])
    await themes(page, banner, info, 'strip-done')
    await banner.getByRole('button', { name: 'Show less', exact: true }).click()
    await expect(banner).toContainText('Done in')
    await themes(page, banner, info, 'line-done')
    // Its name opens the run's page, which draws the whole curve with axes.
    await banner.getByRole('button', { name: /^\W?declared\W?$/ }).click()
    const latest = latestRun(page)
    await expect(latest).toContainText('Done in')
    await expect(latest.getByRole('img', { name: /^loss / })).toHaveAccessibleName(said('loss 0.05'))
    await expect(latest.getByRole('heading', { name: /^\W?loss\W?$/ })).toBeVisible()
    agent.release(hold)
    await expect(page.getByText('Training finished.', { exact: true })).toBeVisible()
  })
})

test('a run with one progress bar and no declared lines shows its bar from the log', async ({ page, project, agent, orx }) => {
  // Two theme passes and the run itself outlast the default budget on a busy machine.
  test.slow()
  const run = payload(orx, `bar-${project.id}`, ['half'], `
const total = 30
for (let i = 0; i <= total; i++) {
  process.stdout.write('\\rtrain: ' + Math.round(100 * i / total) + '%|' + '#'.repeat(i / 3) + '| ' + i + '/' + total + ' [00:' + String(i).padStart(2, '0') + '<00:' + String(total - i).padStart(2, '0') + ', 6.67it/s]')
  // A bar's frame ends where the next begins, so 15 is read once 16 is written.
  if (i === 16) gate('half')
  await tick()
}
process.stdout.write('\\n')`)
  const hold = `bar-result-${project.id}`
  agent.script('finished with status **done**', [{ hold }, { say: 'The bar finished.' }])
  const banner = await launch(page, agent, project, 'Bar', run.command)
  await expect(banner).toContainText(said('Step 15 of 30'))
  await expect(banner.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '50')
  // A bar read from the log times only itself, so its time left is a floor with no range or finishing time.
  await expect(banner).toContainText(/At least ⁨?\d+s⁩? left · Timed from progress in the log; more work may follow it/)
  await expect(banner).not.toContainText('done about')
  await expect(banner.getByRole('progressbar')).toHaveAttribute('aria-valuetext', /At least ⁨?\d+s⁩? left/)
  await run.open('half')
  await agent.reached(hold)
  await expect(banner).toContainText('Done in')
  await expect(banner.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '100')
  agent.release(hold)
  await expect(page.getByText('The bar finished.', { exact: true })).toBeVisible()
})

test('a stage with no total reads as steps and counts the time it has run', async ({ page, project, agent, orx }, info) => {
  // Two theme passes and the run itself outlast the default budget on a busy machine.
  test.slow()
  const run = payload(orx, `open-${project.id}`, ['some'], `
const started = performance.now()
for (let step = 1; step <= 30; step++) {
  await tick()
  process.stdout.write('::orx:: {"step":' + step + ',"t":' + ((performance.now() - started) / 1000) + ',"m":{"loss":' + (2 / step).toFixed(4) + '}}\\n')
  if (step === 24) gate('some')
}`)
  const hold = `open-result-${project.id}`
  agent.script('finished with status **done**', [{ hold }, { say: 'The open run finished.' }])
  const banner = await launch(page, agent, project, 'Open', run.command)
  await expect(banner).toContainText(said('Step 24'))
  await expect(banner).not.toContainText(said('24 of'))
  await expect(banner).toContainText('No estimate')
  await expect(banner.getByRole('progressbar')).toHaveAttribute('aria-valuetext', /elapsed, No estimate[^.]*\.$/)
  await themes(page, banner, info, 'strip-open')
  await run.open('some')
  await agent.reached(hold)
  await expect(banner).toContainText('Done in')
  await expect(banner).not.toContainText('At step')
  await banner.getByRole('button', { name: 'Show less', exact: true }).click()
  await expect(banner).toContainText('Done in')
  await themes(page, banner, info, 'line-open-done')
  agent.release(hold)
  await expect(page.getByText('The open run finished.', { exact: true })).toBeVisible()
})

test('a bar per epoch draws one stage of unknown length, never the finished bars as full', async ({ page, project, agent, orx }, info) => {
  // Two theme passes and the run itself outlast the default budget on a busy machine.
  test.slow()
  const run = payload(orx, `epochs-${project.id}`, ['first', 'third'], `
const items = 6
for (let epoch = 1; epoch <= 5; epoch++) {
  for (let i = 0; i <= items; i++) {
    process.stdout.write('\\rtrain: ' + Math.round(100 * i / items) + '%|' + '#'.repeat(i) + '| ' + i + '/' + items + ' [00:0' + i + '<00:0' + (items - i) + ', 6.67it/s]')
    if (epoch === 3 && i === 4) gate('third')
    await tick()
  }
  process.stdout.write('\\n')
  if (epoch === 1) gate('first')
}`)
  const hold = `epochs-result-${project.id}`
  agent.script('finished with status **done**', [{ hold }, { say: 'The epochs finished.' }])
  const banner = await launch(page, agent, project, 'Epochs', run.command)
  const bar = banner.getByRole('progressbar')
  // A full bar on a run still going says nothing of what remains, however long it waits.
  await expect(banner).toContainText(said('Step 6 of 6'))
  await expect(banner).toContainText('No estimate')
  const elapsed = async () => Number(/(\d+)s⁩? elapsed/.exec(await bar.getAttribute('aria-valuetext') ?? '')?.[1])
  const filled = await elapsed()
  // The strip's clock ticks each second.
  await expect.poll(elapsed, { timeout: 15_000 }).toBeGreaterThan(filled + 3)
  await expect(banner).toContainText('No estimate')
  await expect(banner).not.toContainText('100%')
  await expect(banner).not.toContainText('0s left')
  await expect(bar).not.toHaveAttribute('aria-valuenow')
  await expect(bar.locator('i')).toHaveCount(0)
  await themes(page, banner, info, 'strip-epochs-full')
  await banner.getByRole('button', { name: 'Show less', exact: true }).click()
  // The line's time says it is elapsed, and why there is no estimate.
  await expect(banner).toContainText(/\d+s⁩? elapsed/)
  await expect(banner).not.toContainText('100%')
  await expect(banner.getByRole('button', { name: said('Step 6 of 6') })).toHaveAccessibleDescription('No estimate: how much of this run is left is unknown')
  await themes(page, banner, info, 'line-epochs-full')
  await banner.getByRole('button', { name: 'Show more', exact: true }).click()
  await run.open('first')
  await expect(banner).toContainText(said('Step 3 of 6'))
  await expect(banner).toContainText('No estimate')
  await expect(bar).not.toHaveAttribute('aria-valuenow')
  await expect(bar.locator('> span')).toHaveCount(1)
  await expect(bar.locator('i')).toHaveCount(0)
  await themes(page, banner, info, 'strip-epochs')
  await run.open('third')
  await agent.reached(hold)
  await expect(banner).toContainText('Done in')
  await expect(banner).not.toContainText('At step')
  await expect(bar).toHaveAttribute('aria-valuenow', '100')
  await banner.getByRole('button', { name: 'Show less', exact: true }).click()
  await themes(page, banner, info, 'line-epochs-done')
  agent.release(hold)
  await expect(page.getByText('The epochs finished.', { exact: true })).toBeVisible()
})

/** The text of each drawn row's cell under the column headed `name`, newest first. */
const column = (table: Locator, name: string) => table.evaluate((root, name) => {
  const index = Array.from(root.querySelectorAll('thead th')).findIndex((head) => head.textContent === name)
  return Array.from(root.querySelectorAll<HTMLTableRowElement>('tbody tr:not([aria-hidden])')).map((row) => row.cells[index]?.textContent?.replace(/[⁦-⁩]/g, ''))
}, name)

test("a run that prints a metric and rows shows its chart and its table together, and follows them live to the end", async ({ page, project, agent, orx }, info) => {
  const run = payload(orx, `rounds-${project.id}`, ['third'], `
const total = 8
const started = performance.now()
process.stdout.write('::orx:: {"step":0,"of":' + total + ',"unit":"rounds","t":0}\\n')
for (let round = 1; round <= total; round++) {
  await tick()
  const t = (performance.now() - started) / 1000
  process.stdout.write('::orx:: ' + JSON.stringify({ step: round, t, row: 'rounds', m: { round, win: 0.5 + round / 100, elo: 100 * round, name: 'round ' + round } }) + '\\n')
  process.stdout.write('::orx:: ' + JSON.stringify({ step: round, t, m: { loss: 1 / round } }) + '\\n')
  if (round === 3) gate('third')
}`)
  // The strip is the level this test opens on; tests on this server share the remembered one.
  await page.request.post(`${orx.url}/api/settings/ui-state`, { headers: { origin: orx.url }, data: { runBanner: 'strip' } })
  const hold = `rounds-result-${project.id}`
  agent.script('finished with status **done**', [{ hold }, { say: 'The rounds finished.' }])
  const banner = await launch(page, agent, project, 'Rounds', run.command)

  await test.step('the strip draws the metric the run printed on its own, never a row\'s column', async () => {
    await expect(banner).toContainText(said('Round 3 of 8'))
    await expect(banner.getByRole('img', { name: /^loss / })).toHaveAccessibleName(said('loss 0.333'))
    await expect(banner.getByRole('table')).toHaveCount(0)
  })

  const latest = latestRun(page)
  await test.step('its name opens the page, with its chart and its rows together, and the bar', async () => {
    await banner.getByRole('button', { name: /^\W?rounds\W?$/ }).click()
    await expect(latest.getByRole('heading', { name: /Latest rounds/ })).toBeVisible()
    const table = latest.getByRole('table')
    expect((await table.getByRole('columnheader').allTextContents()).sort()).toEqual(['elo', 'name', 'round', 'win'])
    await expect.poll(() => column(table, 'name')).toEqual(['round 3', 'round 2', 'round 1'])
    await expect(latest.getByRole('img', { name: /^loss / })).toBeVisible()
    // A row's columns live in its table, never as stats beside it.
    await expect(latest.getByRole('term').filter({ hasText: /^(round|win|elo|name)$/ })).toHaveCount(0)
    await expect(latest.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '38')
    await themes(page, latest.getByRole('group', { name: 'rounds' }), info, 'page-rounds')
  })

  await test.step('the page follows the run to its end', async () => {
    await run.open('third')
    await agent.reached(hold)
    await expect(latest).toContainText('Done in')
    await expect.poll(() => column(latest.getByRole('table'), 'name')).toEqual(Array.from({ length: 8 }, (_, i) => `round ${8 - i}`))
    await expect(latest.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '100')
    await themes(page, latest.getByRole('group', { name: 'rounds' }), info, 'page-rounds-done')
    agent.release(hold)
    await expect(page.getByText('The rounds finished.', { exact: true })).toBeVisible()
  })
})

test("every row of a page's table holds still under the reader while rows arrive fast and slide past the cap", async ({ page, project, agent, orx }, info) => {
  const run = payload(orx, `items-${project.id}`, ['live'], `
const note = 'a free-text note long enough to wrap across its column in a narrow pane '.repeat(2)
const row = (i) => '::orx:: ' + JSON.stringify({ step: i, t: i / 100, row: 'items', m: { item: i, name: 'item ' + i, note: i % 7 ? 'ok' : note, score: (i % 13) / 13 } }) + '\\n'
process.stdout.write('::orx:: {"step":0,"of":2060,"unit":"items","t":0}\\n')
let burst = ''
for (let i = 1; i <= 2000; i++) burst += row(i)
process.stdout.write(burst)
gate('live')
for (let i = 2001; i <= 2060; i++) {
  await tick()
  process.stdout.write(row(i))
}`)
  await page.request.post(`${orx.url}/api/settings/ui-state`, { headers: { origin: orx.url }, data: { runBanner: 'strip' } })
  agent.script('finished with status **done**', [{ say: 'The items finished.' }])
  const banner = await launch(page, agent, project, 'Items', run.command)
  await expect.poll(() => column(banner.getByRole('table'), 'name')).toEqual(['item 2000', 'item 1999'])
  await banner.getByRole('button', { name: /^\W?items\W?$/ }).click()
  const latest = latestRun(page)

  const box = latest.getByRole('region', { name: /every row/ })
  await test.step('the page shows the newest rows, and every row in a box a keyboard reaches', async () => {
    await expect.poll(() => column(latest.getByRole('table'), 'name')).toEqual(Array.from({ length: 10 }, (_, i) => `item ${2000 - i}`))
    const all = latest.getByRole('button', { name: /^Show all \W?[\d,]+\W? rows$/ })
    await all.click()
    await expect(box).toBeVisible()
    await page.keyboard.press('Shift+Tab')
    await expect(box).toBeFocused()
  })

  const { x, y } = await box.boundingBox().then((rect) => ({ x: rect!.x + rect!.width / 2, y: rect!.y + rect!.height / 2 }))
  const under = () => page.evaluate(({ x, y }) => {
    const row = document.elementFromPoint(x, y)?.closest('tr')
    return row ? { text: row.textContent, top: Math.round(row.getBoundingClientRect().top) } : null
  }, { x, y })
  const control = latest.getByRole('button', { name: /new rows?$/ })
  const arrived = async () => await control.count() ? Number((await control.textContent())!.replace(/\D/g, '')) : 0

  await test.step('scrolled away from the newest, the row under the pointer stays put while 60 rows arrive', async () => {
    await page.mouse.move(x, y)
    await page.mouse.wheel(0, 3000)
    await expect.poll(() => box.evaluate((element) => element.scrollTop)).toBeGreaterThan(1000)
    // Still once two reads in a row agree.
    let last = ''
    await expect.poll(async () => {
      const now = JSON.stringify(await under())
      const still = now === last
      last = now
      return still
    }).toBe(true)
    const reading = await under()
    expect(reading?.text).toMatch(/item 1\d{3}/)
    await run.open('live')
    // Each frame while rows arrive, including past the 2,048 rows orx keeps, where the oldest leave.
    while (await arrived() < 60) {
      expect(await under()).toEqual(reading)
      await page.evaluate(() => new Promise(requestAnimationFrame))
    }
    expect(await under()).toEqual(reading)
    await expect(control).toHaveText(said('60 new rows'))
    await themes(page, latest.getByRole('group', { name: 'items' }), info, 'page-items-held')
  })

  await test.step('the count returns to the newest rows', async () => {
    await control.click()
    await expect.poll(() => box.evaluate((element) => element.scrollTop)).toBe(0)
    await expect.poll(async () => (await column(box.getByRole('table'), 'name'))[0]).toBe('item 2060')
    await expect(control).toHaveCount(0)
  })
  await expect(page.getByText('The items finished.', { exact: true })).toBeVisible()
})

/** The setup output of a real nanochat run, printed line by line with its own prefixes and colors. */
const nanochat = path.resolve(import.meta.dirname, '../../crates/orx-signals/tests/fixtures/nanochat-setup.out')

/** Replays the nanochat log, stopping at the first line containing each of `stops`. */
const replay = (orx: Orx, name: string, stops: Record<string, string>) => payload(orx, name, Object.keys(stops), `
const lines = readFileSync(${JSON.stringify(nanochat)}, 'utf8').split('\\n')
const stops = ${JSON.stringify(stops)}
for (const line of lines) {
  process.stdout.write(line + '\\n')
  await tick()
  for (const [name, text] of Object.entries(stops)) if (line.includes(text)) gate(name)
}`)

test('a run nobody instrumented says it is setting up and what it printed, then shows the bars its log has', async ({ page, project, agent, orx }, info) => {
  // Two theme passes and the run itself outlast the default budget on a busy machine.
  test.slow()
  const run = replay(orx, `nanochat-${project.id}`, { setup: 'Starting merge loop', tokenizer: '(18202/32503 merges)', training: 'step 00010/01617' })
  const hold = `nanochat-result-${project.id}`
  agent.script('finished with status **done**', [{ hold }, { say: 'The tokenizer and training finished.' }])
  await page.request.post(`${orx.url}/api/settings/ui-state`, { headers: { origin: orx.url }, data: { runBanner: 'strip' } })
  const banner = await launch(page, agent, project, 'Nanochat', run.command)
  const progress = banner.getByRole('progressbar')

  await test.step('before any progress the strip says it is setting up and the newest line, not a fault', async () => {
    await expect(banner).toContainText(/Setting up · \W*Starting merge loop/)
    await expect(banner).not.toContainText('No estimate')
    await expect(banner).not.toContainText('Running on')
    await themes(page, banner, info, 'strip-setting-up')
    await banner.getByRole('button', { name: 'Show less', exact: true }).click()
    await expect(banner).toContainText(/Setting up · \W*Starting merge loop/)
    await themes(page, banner, info, 'line-setting-up')
    await banner.getByRole('button', { name: 'Show more', exact: true }).click()
  })

  await test.step('the tokenizer bar behind its logging prefix is found', async () => {
    await run.open('setup')
    await expect(banner).toContainText(/18,202\W+of\W+32,503\W+merges/)
    await expect(banner).toContainText('At least')
    await themes(page, banner, info, 'strip-tokenizer')
  })

  await test.step('training steps are a second bar, so the whole run has no progress to claim', async () => {
    await run.open('tokenizer')
    await expect(banner).toContainText(said('Step 10 of 1,617'))
    await expect(progress).not.toHaveAttribute('aria-valuenow', /.+/)
    await expect(banner).toContainText('No estimate')
    await themes(page, banner, info, 'strip-training')
  })

  await test.step('the run ends', async () => {
    await run.open('training')
    await agent.reached(hold)
    await expect(banner).toContainText('Done in')
    agent.release(hold)
    await expect(page.getByText('The tokenizer and training finished.', { exact: true })).toBeVisible()
  })
})

test('a run that prints nothing it recognises for a long time is just running, with no estimate and why', async ({ page, project, agent, orx }, info) => {
  test.slow()
  const run = replay(orx, `silent-${project.id}`, { setup: 'Starting merge loop' })
  const hold = `silent-result-${project.id}`
  agent.script('finished with status **done**', [{ hold }, { say: 'The silent run finished.' }])
  await page.request.post(`${orx.url}/api/settings/ui-state`, { headers: { origin: orx.url }, data: { runBanner: 'strip' } })
  const banner = await launch(page, agent, project, 'Silent', run.command)
  await expect(banner).toContainText(/Setting up · \W*Starting merge loop/)
  await page.clock.setFixedTime(Date.now() + 16 * 60_000)
  await expect(banner).toContainText(/Running on/)
  await expect(banner).toContainText(/No estimate: no progress found in this run's log/)
  await themes(page, banner, info, 'strip-silent-for-long')
  await run.open('setup')
  await agent.reached(hold)
  agent.release(hold)
  await expect(page.getByText('The silent run finished.', { exact: true })).toBeVisible()
})

test('several runs share one summary whose rows name what sets each apart and open in place', async ({ page, agent, launchOrx }, info) => {
  // Four runs, three theme passes and each run's result turn outlast the default budget on a busy machine.
  test.slow()
  const orx = await launchOrx()
  await orx.onboard()
  const project = await orx.createProject('sweep')
  /** A run that reports a falling val bpb to `total` steps, held at step 20 until the test opens its gate. */
  const sweep = (name: string, total: number, floor: number) => payload(orx, name, ['mid'], `
const started = performance.now()
process.stdout.write('::orx:: {"step":0,"of":${total},"unit":"steps","t":0}\\n')
for (let step = 1; step <= ${total}; step++) {
  await tick()
  process.stdout.write('::orx:: ' + JSON.stringify({ step, t: (performance.now() - started) / 1000, m: { val_bpb: ${floor} + 1 / step } }) + '\\n')
  if (step === 20) gate('mid')
}`)
  const runs = [
    { title: 'Depth 4 at 2e15 FLOPs', label: 'Depth 4', total: 40, run: sweep('depth-4', 40, 1.3) },
    { title: 'Depth 6 at 2e15 FLOPs', label: 'Depth 6', total: 60, run: sweep('depth-6', 60, 1.2) },
    { title: 'Depth 8 at 2e15 FLOPs', label: 'Depth 8', total: 80, run: sweep('depth-8', 80, 1.25) },
    { title: 'Tokenizer check', label: 'Tokenizer check', total: 30, run: sweep('tokenizer', 30, 4) },
  ]
  const start = (from: number, to: number) => runs.slice(from, to).flatMap(({ title, run }) => [
    `id=$(orx create-experiment ${project.id} --title '${title}' --run-command '${run.command}' | sed -n 's/^ *id: *//p')`,
    'orx exp run "$id" --backend local',
    'orx exp wake "$id"',
  ])
  agent.script('Run the depth sweep.', [{ run: ['set -e', ...start(0, 3)].join('\n') }, { say: 'Launched the sweep.' }])
  await project.open(page)
  await send(page, 'Run the depth sweep.')
  const banner = page.getByRole('region', { name: 'Runs in this chat' })
  const rows = banner.getByRole('list').getByRole('listitem')
  const summary = banner.getByRole('button', { name: /^(Show each run|Show less)$/ }).first()
  const named = (title: string) => banner.getByRole('button', { name: new RegExp(`^\\W?${title}\\W?$`) })

  await test.step('the summary says what the runs share, and each row only what sets it apart', async () => {
    await expect(rows).toHaveCount(3)
    for (const { total } of runs.slice(0, 3)) await expect(banner).toContainText(said(`Step 20 of ${total}`))
    await expect(banner).toContainText('… at 2e15 FLOPs')
    await expect(banner).toContainText(said('3 running'))
    for (const [index, { title, label }] of runs.slice(0, 3).entries()) {
      await expect(named(title)).toHaveText(said(label))
      await expect(rows.nth(index)).toContainText(said(label))
    }
    await themes(page, banner, info, 'summary-rows')
  })

  await test.step('the summary folds to one line over every bar, and opens back to its rows', async () => {
    await banner.getByRole('button', { name: 'Show less', exact: true }).click()
    await expect(rows).toHaveCount(0)
    await expect(banner.getByRole('progressbar')).toHaveCount(3)
    await expect(summary).toBeFocused()
    await themes(page, banner, info, 'summary-folded')
    await summary.click()
    await expect(rows).toHaveCount(3)
  })

  await test.step('a row opens into its strip in place, and Escape steps back to the row', async () => {
    await rows.nth(1).getByRole('button', { name: 'Show more', exact: true }).click()
    await expect(rows.nth(1).getByRole('img', { name: /^val_bpb / })).toBeVisible()
    await expect(rows).toHaveCount(3)
    await themes(page, banner, info, 'summary-row-strip')
    await page.keyboard.press('Escape')
    await expect(rows.nth(1).getByRole('button', { name: 'Show more', exact: true })).toBeFocused()
  })

  await test.step('an unrelated run keeps its whole title and leaves the sweep its labels', async () => {
    agent.script('Also check the tokenizer.', [{ run: ['set -e', ...start(3, 4)].join('\n') }, { say: 'Launched the check.' }])
    await send(page, 'Also check the tokenizer.')
    await expect(rows).toHaveCount(4)
    await expect(banner).toContainText(said('Step 20 of 30'))
    await expect(banner).not.toContainText('… at 2e15 FLOPs')
    for (const { title, label } of runs) await expect(named(title)).toHaveText(said(label))
    await expect(banner).toContainText(said('4 running'))
  })

  await test.step('runs finish one at a time, each answered in the chat', async () => {
    const shown: { run: { id: string }; experimentName: string }[] = await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json()
    for (const { title, run } of runs) {
      const id = shown.find((item) => item.experimentName === title)!.run.id
      agent.script(`Run \`${id}\` finished`, [{ say: `${title} is done.` }])
      await run.open('mid')
      await expect(page.getByText(`${title} is done.`, { exact: true })).toBeVisible()
    }
  })
})

test('a rule stops a run on its third NaN, and the agent repairing it reads Revising until its turn ends', async ({ page, project, agent, orx }, info) => {
  // Three theme passes and the run itself outlast the default budget on a busy machine.
  test.slow()
  const run = payload(orx, `ruled-${project.id}`, ['nan'], `
process.stdout.write('::orx:: {"step":0,"of":400,"unit":"steps","t":0}\\n')
for (let step = 1; step <= 400; step++) {
  await tick()
  if (step === 10) gate('nan')
  process.stdout.write('::orx:: {"step":' + step + ',"m":{"loss":' + (step >= 10 ? 'NaN' : (2 / step).toFixed(4)) + '}}\\n')
}`)
  const rules = path.join(orx.root, `rules-${project.id}.json`)
  writeFileSync(rules, JSON.stringify({ rules: [{ name: 'nan-loss', text: 'Stop when the loss is NaN on 3 reports in a row.', does: 'stop', when: { nonFinite: 'loss', reports: 3 } }] }))
  agent.script('Launch Ruled.', [
    { run: [
      'set -e',
      `id=$(orx create-experiment ${project.id} --title Ruled --run-command '${run.command}' | sed -n 's/^ *id: *//p')`,
      `orx exp view set "$id" --config ${rules}`,
      'orx exp run "$id" --backend local',
      'orx exp wake "$id"',
    ].join('\n') },
    { say: 'Launched Ruled.' },
  ])
  await project.open(page)
  await send(page, 'Launch Ruled.')
  const banner = page.getByRole('region', { name: 'Runs in this chat' })
  await expect(banner.getByRole('button', { name: 'Stop the run', exact: true })).toBeVisible()
  await expect(banner).toContainText(said('Step 9 of 400'))
  const shown = (await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json()).find((item: { run: { projectId: string } }) => item.run.projectId === project.id)
  const { id: runId, experimentId } = shown.run

  const [seen, repairing] = [`seen-${project.id}`, `repairing-${project.id}`]
  agent.script('finished with status', [
    { hold: seen },
    { run: `orx exp revise ${experimentId} --run ${runId}` },
    { hold: repairing },
    { say: 'Added warm-up; relaunching next.' },
  ])
  await run.open('nan')

  await test.step('the third NaN stops the run, named by its rule, with no model involved', async () => {
    await agent.reached(seen)
    await expect(banner).toContainText('Auto-stopped')
    await expect.poll(async () => (await banner.innerText()).replace(/[⁦-⁩]/g, ''))
      .toContain('Stop when the loss is NaN on 3 reports in a row. Rule: nan-loss. No model was involved.')
    const { statusText } = await (await page.request.get(`${orx.url}/api/runs/${runId}/monitor`)).json()
    // A pass counts every report since the last, so a fast run is stopped a few reports past the third.
    expect(statusText).toMatch(/\nstop {5}Auto-stopped by rule nan-loss: loss was NaN on \d+ reports in a row, step 10 to \d+\. No model was involved\.\n/)
    await themes(page, banner, info, 'strip-auto-stopped')
  })

  await test.step('the agent marks the run it is repairing, which reads Revising until its turn ends', async () => {
    agent.release(seen)
    await agent.reached(repairing)
    await expect(banner).toContainText('Revising')
    await expect(banner.getByRole('button', { name: 'Dismiss', exact: true })).toHaveCount(0)
    await themes(page, banner, info, 'strip-revising')
    agent.release(repairing)
    await expect(page.getByText('Added warm-up; relaunching next.', { exact: true })).toBeVisible()
    await expect.poll(async () => {
      const { run: stopped } = await (await page.request.get(`${orx.url}/api/runs/${runId}`)).json()
      return [stopped.outcome?.by, stopped.outcome?.rule?.name, stopped.monitor?.tone]
    }).toEqual(['rule', 'nan-loss', 'stopped'])
  })
})

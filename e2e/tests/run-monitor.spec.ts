import { spawnSync } from 'node:child_process'
import { writeFile } from 'node:fs/promises'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import type { Locator, Page, TestInfo } from '@playwright/test'
import type { Agent } from '../src/agent.ts'
import type { Orx, Project } from '../src/orx.ts'
import { expect as baseExpect, composer, send, test } from '../src/test.ts'

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
      // Shimmering text paints through its background gradient, which this check can't read.
      if (style.backgroundClip === 'text' || style.webkitBackgroundClip === 'text') continue
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

async function openRunPage(page: Page, banner: Locator, name: RegExp) {
  await banner.getByRole('button', { name }).hover()
  await page.getByRole('dialog').getByRole('link', { name }).click()
}

/** Text as the page writes it, numbers wrapped in direction isolates. */
const said = (text: string) => new RegExp(text.replace(/\d+/g, (n) => `[⁦⁨]?${n}[⁩]?`))

const steps = async (page: Page, orx: Orx, runId: string) => {
  const monitor = await (await page.request.get(`${orx.url}/api/runs/${runId}/monitor`)).json()
  return { monitor, x: (monitor.series.loss ?? []).map(([x]: [number]) => x) as number[] }
}

test('run chart settings save, redraw and follow the run into chat', async ({ page, project, agent, orx }) => {
  test.slow()
  const run = payload(orx, `chart-settings-${project.id}`, ['finish'], `
const loss = [1, 0.9, 0.6, 0.8, 0.3, 0.2, 0.26, 0.13, 0.17, 0.09, 0.05, 0.08]
for (const [i, value] of loss.entries()) console.log('::orx:: ' + JSON.stringify({ step: i + 1, of: 16, m: { loss: value } }))
gate('finish')`)
  const view = path.join(orx.root, `chart-view-${project.id}.xml`)
  writeFileSync(view, '<signal name="loss" label="Training loss" smooth="0.25"/>\n<view headline="loss"><chart y="loss"/></view>')
  await page.request.post(`${orx.url}/api/settings/ui-state`, { headers: { origin: orx.url }, data: { runBanner: 'strip' } })
  agent.script('Start training.', [
    { run: [
      'set -e',
      `id=$(orx create-experiment ${project.id} --title Training --run-command '${run.command}' | sed -n 's/^ *id: *//p')`,
      `orx exp view set "$id" --file ${view}`,
      'orx exp run "$id" --backend local',
      'orx exp wake "$id"',
    ].join('\n') },
    { say: 'Training started.' },
  ])
  agent.script('finished with status **done**', [{ say: 'Training finished.' }])
  await project.open(page)
  await send(page, 'Start training.')
  const banner = page.getByRole('region', { name: 'Runs in this chat' })
  const strip = banner.getByRole('group', { name: /Training loss/ })
  const menu = page.getByRole('menu', { name: 'Chart settings' })
  const choice = (axis: string, name: string) => menu.getByRole('group', { name: axis }).getByRole('menuitemradio', { name, exact: true })
  const open = async (chart: Locator) => {
    await chart.click({ button: 'right' })
    await expect(menu).toBeVisible()
  }
  const capture = async (name: string, chart: Locator, withMenu = false) => {
    const dir = process.env.ORX_E2E_SHOTS
    if (!dir) return
    mkdirSync(dir, { recursive: true })
    for (const width of [1440, 420]) {
      await page.setViewportSize({ width, height: 900 })
      await chart.scrollIntoViewIfNeeded()
      const plot = await chart.boundingBox()
      if (withMenu) await open(chart)
      else await chart.blur()
      const popup = withMenu ? await menu.boundingBox() : null
      if (!plot || (withMenu && !popup)) throw new Error('The chart or menu is hidden in the capture')
      const y = Math.max(0, Math.floor(Math.min(plot.y - 36, popup ? popup.y - 8 : Infinity)))
      for (const scheme of ['light', 'dark'] as const) {
        await page.emulateMedia({ colorScheme: scheme })
        await expect(page.locator('html')).toHaveAttribute('data-theme', scheme)
        await expect.poll(() => page.evaluate(() => document.getAnimations().filter((a) => a instanceof CSSTransition).length)).toBe(0)
        if (withMenu) expect(await contrast(menu)).toEqual([])
        await page.screenshot({ path: path.join(dir, `${name}-${width}-${scheme}.png`), clip: { x: 0, y, width, height: 900 - y } })
      }
      await page.emulateMedia({ colorScheme: 'light' })
      if (withMenu) await page.keyboard.press('Escape')
    }
    await page.setViewportSize({ width: 1440, height: 900 })
  }
  await expect(strip.getByRole('img')).toBeVisible()
  await capture('composer-menu', strip, true)
  if (process.platform === 'darwin') {
    await strip.click({ modifiers: ['Control'] })
    await expect(menu).toBeVisible()
    await page.keyboard.press('Escape')
  }

  await openRunPage(page, banner, /Training/)
  const chart = latestRun(page).getByRole('group', { name: /Training loss/ })
  const image = chart.getByRole('img')
  const raw = image.locator('path[opacity="0.3"]')
  await expect(raw).toHaveCount(1)
  await capture('run-menu', chart, true)
  await capture('before-log', chart)

  await chart.press('Shift+F10')
  await expect(menu).toBeVisible()
  await expect(choice('Y axis', 'Linear')).toHaveAttribute('aria-checked', 'true')
  await page.keyboard.press('Home')
  await expect(choice('Y axis', 'Linear')).toBeFocused()
  await page.keyboard.press('ArrowDown')
  await expect(choice('Y axis', 'Logarithmic')).toBeFocused()
  await page.keyboard.press('Enter')
  await expect(menu).toBeHidden()
  await expect(chart).toBeFocused()
  await expect(image.getByText('log', { exact: true })).toBeVisible()
  await expect(strip.getByText(/\blog\b/)).toBeVisible()
  await capture('after-log', chart)
  await capture('before-smoothing-off', chart)

  await chart.press('ContextMenu')
  await expect(choice('Smoothing', 'Medium')).toHaveAttribute('aria-checked', 'true')
  await choice('Smoothing', 'Off').click()
  await expect(menu).toBeHidden()
  await expect(raw).toHaveCount(0)
  await expect(strip.getByRole('img').locator('path[opacity="0.3"]')).toHaveCount(0)
  await capture('after-smoothing-off', chart)

  await page.route('**/api/runs/*/view/chart', (route) => route.fulfill({
    status: 409,
    json: { error: "The experiment's view changed. Choose the chart setting again." },
  }), { times: 1 })
  await open(chart)
  await choice('Y axis', 'Linear').click()
  await expect(menu.getByRole('alert')).toContainText('The experiment\'s view changed.')
  await expect(choice('Y axis', 'Logarithmic')).toHaveAttribute('aria-checked', 'true')
  await page.keyboard.press('Escape')
  await expect(chart).toBeFocused()
  await expect(image.getByText('log', { exact: true })).toBeVisible()

  const shown = (await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json()).find((item: { run: { projectId: string } }) => item.run.projectId === project.id)
  const { monitor } = await steps(page, orx, shown.run.id)
  expect(monitor.viewFrom.markup).toContain('y-scale="log"')
  expect(monitor.viewFrom.markup).not.toContain('smooth=')
  await page.reload()
  await open(strip)
  await expect(choice('Y axis', 'Logarithmic')).toHaveAttribute('aria-checked', 'true')
  await expect(choice('Smoothing', 'Off')).toHaveAttribute('aria-checked', 'true')
  await page.keyboard.press('Escape')
  await run.open('finish')
  await expect(page.getByText('Training finished.', { exact: true })).toBeVisible()
})

test('Stop asks before cancelling a running experiment', async ({ page, project, agent, orx }) => {
  const run = payload(orx, 'stop-run', ['finish'], "gate('finish')")
  agent.script('Stopped by: the user.', [{ say: 'The run was stopped.' }])
  const banner = await launch(page, agent, project, 'Training', run.command)
  const stop = banner.getByRole('button', { name: 'Stop the run', exact: true })
  const dialog = page.getByRole('dialog', { name: /Stop.*Training/ })
  const cancellations: string[] = []
  page.on('request', request => {
    if (request.method() === 'POST' && request.url().endsWith('/cancel')) cancellations.push(request.url())
  })

  await stop.click()
  await expect(dialog.getByRole('button', { name: 'Cancel', exact: true })).toBeFocused()
  await page.keyboard.press('Escape')
  await expect(dialog).toBeHidden()
  await expect(stop).toBeFocused()
  expect(cancellations).toEqual([])

  await stop.click()
  await dialog.getByRole('button', { name: 'Stop run', exact: true }).click()
  await expect(dialog).toBeHidden()
  await expect(page.getByText('The run was stopped.', { exact: true })).toBeVisible()
  expect(cancellations).toHaveLength(1)
})

test('planned finalization keeps completed training from claiming zero time left', async ({ page, project, agent, orx }, info) => {
  test.slow()
  const run = payload(orx, 'finalization', ['validation', 'checkpoint', 'save'], `
console.log('::orx:: {"plan":[{"phase":"train","of":2,"unit":"steps"},{"phase":"train-finalize","of":2,"unit":"tasks"}]}')
console.log('::orx:: {"phase":"train","step":0,"of":2,"unit":"steps"}')
console.log('::orx:: {"step":2,"m":{"val_bpb":1.8}}')
gate('validation')
console.log('::orx:: {"phase":"train-finalize","step":0,"of":2,"unit":"tasks"}')
gate('checkpoint')
console.log('::orx:: {"step":1}')
gate('save')
console.log('::orx:: {"step":2}')
`)
  try {
    const hold = `finalization-result-${project.id}`
    agent.script('finished with status **done**', [{ hold }, { say: 'Finalization finished.' }])
    const banner = await launch(page, agent, project, 'Finalization', run.command)
    const { experimentId } = (await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json())[0].run
    await expect(banner).toContainText(said('2 of 2'))
    await expect(banner).not.toContainText(said('0s left'))
    await run.open('validation')
    await expect(banner).toContainText(said('0 of 2'))
    await expect(banner).toContainText('tasks')
    await expect(banner).not.toContainText(said('0s left'))
    console.log('Finalization line:', await banner.innerText())
    await themes(page, banner, info, 'finalization-unknown-duration')
    await run.open('checkpoint')
    await run.open('save')
    await agent.reached(hold)
    await expect(banner).toContainText('Done in')
    agent.release(hold)
    await expect(page.getByText('Finalization finished.', { exact: true })).toBeVisible()

    const repeated = `finalization-repeated-${project.id}`
    agent.script('Repeat Finalization.', [
      { run: `orx exp run ${experimentId} --backend local` },
      { say: 'Repeated Finalization.' },
    ])
    agent.script('finished with status **done**', [{ hold: repeated }, { say: 'Repeated finalization finished.' }])
    await send(page, 'Repeat Finalization.')
    await expect(banner.getByRole('button', { name: 'Stop the run', exact: true })).toBeVisible()
    await run.open('validation')
    await expect(banner).toContainText(said('0 of 2'))
    await run.open('checkpoint')
    await expect(banner).toContainText(said('1 of 2'))
    // The parent finished this stage almost at once, so the repeat is soon overdue against it.
    await expect(banner).toContainText(/\d+[smh][⁦-⁩]* left|No progress for/)
    await expect(banner).not.toContainText(/0s[⁦-⁩]* left/)
    console.log('Finalization with prior:', await banner.innerText())
    await themes(page, banner, info, 'finalization-with-prior')
    await run.open('save')
    await agent.reached(repeated)
    await expect(banner).toContainText('Done in')
    agent.release(repeated)
  } finally {
    const { runs } = await (await page.request.get(`${orx.url}/api/projects/${project.id}/runs`)).json()
    for (const run of runs) {
      await page.request.post(`${orx.url}/api/runs/${run.id}/cancel`, { headers: { origin: orx.url } })
      await expect.poll(async () => {
        const { run: current } = await (await page.request.get(`${orx.url}/api/runs/${run.id}`)).json()
        return current.status
      }).toMatch(/^(done|failed|cancelled)$/)
    }
  }
})

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
  // The NaN flags the run; the agent it wakes reads which run from the file the test writes once it knows.
  const [woken, ids] = [`woken-${project.id}`, path.join(orx.root, 'declared-ids')]
  agent.script('was flagged for review', [
    { hold: woken },
    { run: `read run exp < ${ids}; ${settle('"$exp"', '"$run"', 'keep', 'One NaN at step 10; the steps after it are numbers again.')}` },
    { say: 'One NaN at step 10, numbers again after it. Keeping it going.' },
  ])
  const banner = await launch(page, agent, project, 'Declared', run.command)
  const chart = banner.getByRole('img', { name: /^loss / })
  const { id: runId, experimentId } = (await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json())[0].run

  await test.step('a NaN reads NaN, flags the run, and the agent it wakes keeps it going', async () => {
    await agent.reached(woken)
    const status = banner.getByRole('button', { name: /Declared/ })
    await expect(status).toHaveText(/^[⁨]?loss became NaN at step 10\.[⁩]?$/)
    await status.hover()
    await expect(page.getByRole('dialog')).toContainText(said('Step 10 of 40'))
    await page.keyboard.press('Escape')
    writeFileSync(ids, `${runId} ${experimentId}\n`)
    agent.release(woken)
    await expect(page.getByText('One NaN at step 10, numbers again after it. Keeping it going.', { exact: true })).toBeVisible()
    await expect(chart).toHaveAccessibleName('loss NaN')
    await expect(banner).toContainText('NaN')
  })

  await test.step('half way the bar and the chart carry progress, with time left and the latest value', async () => {
    await run.open('nan')
    await expect(banner).toContainText(said('Step 20 of 40'))
    await expect(banner.getByRole('progressbar')).toHaveCount(1)
    await expect(banner).toContainText(/\d+s⁩? left/)
    await expect(banner).not.toContainText('At least')
    await expect(chart).toHaveAccessibleName(said('loss 0.1'))
    await themes(page, banner, info, 'strip-running')
  })

  await test.step('the run goes on while orx up is down; the open page catches up with each step once', async () => {
    await orx.restart('SIGTERM', () => run.open('half'))
    await expect(banner).toContainText(said('Step 30 of 40'), { timeout: 20_000 })
    await expect(banner.getByRole('progressbar')).toHaveCount(1)
    await expect(chart).toHaveAccessibleName(said('loss 0.0667'))
    const { x } = await steps(page, orx, runId)
    expect(x).toEqual(Array.from({ length: 30 }, (_, i) => i + 1))
    await run.open('late')
  })

  await test.step('the run ends and the line reads done', async () => {
    await agent.reached(hold)
    await expect(banner).toContainText('Done in')
    await expect(banner.getByRole('progressbar')).toHaveCount(0)
    const { monitor, x } = await steps(page, orx, runId)
    expect(x).toEqual(Array.from({ length: 40 }, (_, i) => i + 1))
    expect(monitor.series.loss[9]).toEqual([10, null])
    await themes(page, banner, info, 'strip-done')
    await banner.getByRole('button', { name: 'Show less', exact: true }).click()
    await expect(banner).toContainText('Done in')
    await themes(page, banner, info, 'line-done')
    await banner.getByRole('button', { name: 'Show more', exact: true }).click()
    // The status card links to the run page, which draws the whole curve with axes.
    await openRunPage(page, banner, /Declared/)
    const latest = latestRun(page)
    await expect(latest).toContainText('Done in')
    await expect(latest.getByRole('img', { name: /^loss / })).toHaveAccessibleName(said('loss 0.05'))
    await expect(latest.getByRole('heading', { name: /^\W?loss\W?$/ })).toBeVisible()
    agent.release(hold)
    await expect(page.getByText('Training finished.', { exact: true })).toBeVisible()
  })
})

test('a named counter ends setup before the run charts anything', async ({ page, project, agent, orx }) => {
  const run = payload(orx, `counts-${project.id}`, ['count', 'finish'], `
console.log('::orx:: {"plan":[{"phase":"train","of":20}]}')
console.log('::orx:: {"phase":"train"}')
gate('count')
console.log('::orx:: {"step":0,"t":0}')
console.log('::orx:: {"step":2,"t":10}')
gate('finish')
console.log('::orx:: {"step":20,"t":100}')
`)
  const hold = `counts-result-${project.id}`
  agent.script('finished with status **done**', [{ hold }, { say: 'Counting finished.' }])
  const banner = await launch(page, agent, project, 'Counting', run.command)
  const { id } = (await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json())[0].run
  try {
    const status = banner.getByRole('button', { name: /Counting/ })
    await expect(status).toContainText('Setup')
    await run.open('count')

    const stage = /train[⁦-⁩]* \([⁦-⁩]*2\/2[⁦-⁩]*\)/
    const count = said('Step 2 of 20')
    await expect(status).toContainText(stage)
    await expect(status).toContainText(count)
    await expect(banner).toContainText('left')
    await status.hover()
    const card = page.getByRole('dialog')
    const current = card.locator('[aria-current="step"]')
    await expect(current).toContainText('train')
    await expect(current).toContainText(count)
    await expect(card).toContainText(/Whole run: .*left/)
    await page.keyboard.press('Escape')
    await expect(card).toHaveCount(0)
    await openRunPage(page, banner, /Counting/)
    const latest = latestRun(page)
    await expect(latest).toContainText(stage)
    await expect(latest).toContainText(count)
    await expect(latest).toContainText('left')

    await run.open('finish')
    await agent.reached(hold)
    await expect(latest).toContainText('Done in')
    agent.release(hold)
  } finally {
    const { run: current } = await (await page.request.get(`${orx.url}/api/runs/${id}`)).json()
    if (current.status === 'running') {
      await page.request.post(`${orx.url}/api/runs/${id}/cancel`, { headers: { origin: orx.url } })
    }
  }
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
  await expect(banner.getByRole('progressbar')).not.toHaveAttribute('aria-valuenow')
  await expect(banner).not.toContainText('done about')
  await expect(banner).not.toContainText('left')
  await expect.poll(() => banner.getByRole('progressbar').locator('i').evaluate((element) => element.style.width)).toBe('50%')
  await run.open('half')
  await agent.reached(hold)
  await expect(banner).toContainText('Done in')
  await expect(banner.getByRole('progressbar')).toHaveCount(0)
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
  agent.script('needs an instrumentation check', [{ say: 'This run deliberately has no total.' }])
  const hold = `open-result-${project.id}`
  agent.script('finished with status **done**', [{ hold }, { say: 'The open run finished.' }])
  const banner = await launch(page, agent, project, 'Open', run.command)
  await expect(banner).toContainText(said('Step 24'))
  await expect(banner).not.toContainText(said('24 of'))
  await expect(banner).not.toContainText('No estimate')
  await expect(banner.getByRole('progressbar')).not.toHaveAttribute('aria-valuenow')
  await expect(page.getByText('This run deliberately has no total.', { exact: true })).toBeVisible()
  await themes(page, banner, info, 'strip-open')
  await run.open('some')
  await agent.reached(hold)
  await expect(banner).toContainText('Done in')
  await expect(banner).not.toContainText('At step')
  await expect(banner.getByRole('button', { name: 'Show less', exact: true })).toBeVisible()
  await themes(page, banner, info, 'strip-open-done')
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
  await expect(banner).toContainText(said('Step 6 of 6'))
  await expect(banner).toContainText('elapsed')
  await expect(banner).not.toContainText('Setup')
  await expect(banner).not.toContainText('100%')
  await expect(banner).not.toContainText('0s left')
  await expect(bar).not.toHaveAttribute('aria-valuenow')
  await expect(bar.locator('> span')).toHaveCount(1)
  await themes(page, banner, info, 'row-epochs-full')
  await run.open('first')
  await expect(banner).toContainText(said('Step 3 of 6'))
  await expect(banner).toContainText('elapsed')
  await expect(banner).not.toContainText('Setup')
  await expect(bar).not.toHaveAttribute('aria-valuenow')
  await expect(bar.locator('> span')).toHaveCount(1)
  await expect(bar.locator('i')).toHaveCount(1)
  await themes(page, banner, info, 'strip-epochs')
  await run.open('third')
  await agent.reached(hold)
  await expect(banner).toContainText('Done in')
  await expect(banner).not.toContainText('At step')
  await expect(bar).toHaveCount(0)
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
    await openRunPage(page, banner, /Rounds/)
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
    await expect(latest.getByRole('progressbar')).toHaveCount(0)
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
  await expect(banner.getByRole('table')).toHaveCount(0)
  await openRunPage(page, banner, /Items/)
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
  for (const [name, text] of Object.entries(stops)) if (line.includes(text)) gate(name)
}`)

test('an uninstrumented run shows what it printed, then shows the bars its log has', async ({ page, project, agent, orx }, info) => {
  // Two theme passes and the run itself outlast the default budget on a busy machine.
  test.slow()
  const run = replay(orx, `nanochat-${project.id}`, { setup: 'Starting merge loop', tokenizer: '(18202/32503 merges)', training: 'step 00010/01617' })
  const hold = `nanochat-result-${project.id}`
  agent.script('finished with status **done**', [{ hold }, { say: 'The tokenizer and training finished.' }])
  await page.request.post(`${orx.url}/api/settings/ui-state`, { headers: { origin: orx.url }, data: { runBanner: 'strip' } })
  const banner = await launch(page, agent, project, 'Nanochat', run.command)
  const progress = banner.getByRole('progressbar')

  await test.step('before the first bar the line shows elapsed time and the newest log line', async () => {
    await expect(banner).toContainText(/Running\W+Starting merge loop/)
    await expect(banner).toContainText('elapsed')
    await expect(banner).not.toContainText('No estimate')
    await expect(banner).not.toContainText('Running on')
    await themes(page, banner, info, 'line-before-tokenizer')
  })

  await test.step('the tokenizer bar behind its logging prefix is found', async () => {
    await run.open('setup')
    await expect(banner).toContainText(/18,202\W+of\W+32,503\W+merges/)
    await expect(banner).not.toContainText('Setup')
    await expect(banner).not.toContainText('left')
    await themes(page, banner, info, 'strip-tokenizer')
  })

  await test.step('training steps are a second bar, so the whole run has no progress to claim', async () => {
    await run.open('tokenizer')
    await expect(banner).toContainText(said('Step 10 of 1,617'))
    await expect(progress).not.toHaveAttribute('aria-valuenow', /.+/)
    await expect(banner).toContainText('elapsed')
    await expect(banner).not.toContainText('Setup')
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

test('a plain run shows elapsed time without inventing a stage', async ({ page, project, agent, orx }, info) => {
  test.slow()
  const run = payload(orx, `plain-${project.id}`, ['done'], `
console.log('Finished checking the rowan sample')
gate('done')
`)
  const hold = `plain-result-${project.id}`
  agent.script('finished with status **done**', [{ hold }, { say: 'The plain run finished.' }])
  await page.request.post(`${orx.url}/api/settings/ui-state`, { headers: { origin: orx.url }, data: { runBanner: 'strip' } })
  const banner = await launch(page, agent, project, 'Plain', run.command)
  const status = banner.getByRole('button', { name: /Running.+Plain/ })
  const elapsed = /2m[⁦-⁩]* elapsed/
  await expect(status).toHaveText(/Running\W+Finished checking the rowan sample/)
  await page.clock.setFixedTime(Date.now() + 2 * 60_000)
  await expect(banner).toContainText(elapsed)

  const card = page.getByRole('dialog', { name: 'Plain' })
  for (const width of [1440, 420]) {
    await page.setViewportSize({ width, height: 860 })
    await themes(page, banner, info, `plain-line-${width}`)
    await status.hover()
    await expect(card).toContainText(elapsed)
    await expect(card.getByRole('list')).toHaveCount(0)
    await themes(page, card, info, `plain-card-${width}`)
    await page.mouse.move(0, 0)
    await expect(card).toBeHidden()
  }

  await openRunPage(page, banner, /Plain/)
  const details = latestRun(page).getByRole('group', { name: 'Plain' })
  await expect(details).toContainText('Running')
  await expect(details).toContainText(elapsed)
  await expect(details.getByRole('list')).toHaveCount(0)
  for (const width of [420, 1440]) {
    await page.setViewportSize({ width, height: 860 })
    await themes(page, details, info, `plain-page-${width}`)
  }

  const watch = page.locator('section', { has: page.getByRole('heading', { name: 'Watch', exact: true }) })
  await expect(watch).toContainText("This chat's agent has no watching model")
  await expect(watch.getByRole('button', { name: 'Look now' })).toHaveCount(0)
  await themes(page, watch, info, 'watch-no-model')

  await run.open('done')
  await agent.reached(hold)
  await expect(details).toContainText('Done in')
  agent.release(hold)
  await expect(page.getByText('The plain run finished.', { exact: true })).toBeVisible()
})

test('several runs share one summary over an open card each, whose rows name what sets each apart', async ({ page, agent, launchOrx }, info) => {
  // Four runs, three theme passes and each run's result turn outlast the default budget on a busy machine.
  test.slow()
  const orx = await launchOrx()
  await orx.onboard()
  const project = await orx.createProject('sweep')
  /** A falling val bpb every two simulated seconds, held at step 20 while its estimate counts down. */
  const sweep = (name: string, total: number, floor: number) => payload(orx, name, ['mid'], `
process.stdout.write('::orx:: {"step":0,"of":${total},"unit":"steps","t":0}\\n')
for (let step = 1; step <= ${total}; step++) {
  await tick()
  process.stdout.write('::orx:: ' + JSON.stringify({ step, t: step * 2, m: { val_bpb: ${floor} + 1 / step } }) + '\\n')
  if (step === 20) gate('mid')
}`)
  const runs = [
    { title: 'Depth 4 at 2e15 FLOPs', label: '4', total: 40, run: sweep('depth-4', 40, 1.3) },
    { title: 'Depth 6 at 2e15 FLOPs', label: '6', total: 60, run: sweep('depth-6', 60, 1.2) },
    { title: 'Depth 8 at 2e15 FLOPs', label: '8', total: 80, run: sweep('depth-8', 80, 1.25) },
    { title: 'Tokenizer check', label: 'Tokenizer check', total: 30, run: sweep('tokenizer', 30, 4) },
  ]
  const start = (from: number, to: number) => runs.slice(from, to).flatMap(({ title, run }) => [
    `id=$(orx create-experiment ${project.id} --title '${title}' --run-command '${run.command}' | sed -n 's/^ *id: *//p')`,
    'orx exp run "$id" --backend local',
    'orx exp wake "$id"',
  ])
  agent.script('Run the depth sweep.', [{ run: ['set -e', ...start(0, 3)].join('\n') }, { say: 'Launched the sweep.' }])
  await page.setViewportSize({ width: 1440, height: 1100 })
  await project.open(page)
  await send(page, 'Run the depth sweep.')
  await expect(page.getByText('Launched the sweep.', { exact: true })).toBeVisible({ timeout: 90_000 })
  const banner = page.getByRole('region', { name: 'Runs in this chat' })
  const rows = banner.getByRole('list').getByRole('listitem')
  const summary = banner.getByRole('button', { name: /^(Show each run|Show less)$/ }).first()
  const named = (title: string) => banner.getByRole('button', { name: new RegExp(title) })

  await test.step('the summary says what the runs share, and each row only what sets it apart', async () => {
    await expect(rows).toHaveCount(3, { timeout: 60_000 })
    for (const { total } of runs.slice(0, 3)) await expect(banner).toContainText(said(`Step 20 of ${total}`))
    await expect(banner).toContainText(said('Depth 4, 6, 8 at 2e15 FLOPs'))
    await expect(banner).toContainText(said('3 running'))
    for (const [index, { title, label }] of runs.slice(0, 3).entries()) {
      await expect(named(title)).toContainText(said(label))
      await expect(rows.nth(index)).toContainText(said(label))
      await expect(rows.nth(index).getByRole('img', { name: /^val_bpb / })).toBeVisible()
    }
    await themes(page, banner, info, 'summary-rows')

    const shown = await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json()
    const finishes = await Promise.all(shown.map(async ({ run }: { run: { id: string } }) => {
      const monitor = await (await page.request.get(`${orx.url}/api/runs/${run.id}/monitor`)).json()
      const { at, midMs, hiMs } = monitor.estimate
      expect(midMs).toBeGreaterThan(0)
      return { mid: at + midMs, high: at + (hiMs ?? midMs) }
    }))
    const firstFinish = Math.min(...finishes.map(({ mid }) => mid))
    const estimate = banner.locator('[data-part="estimate"]')
    await page.clock.setFixedTime(firstFinish - 10_500)
    await expect(estimate).toContainText(said('10s'))
    for (const width of [1440, 420, 320]) {
      await page.setViewportSize({ width, height: 860 })
      for (const name of ['shared', 'sentence', 'estimate']) {
        const part = banner.locator(`[data-part="${name}"]`)
        await expect(part).toBeVisible()
        const overflow = await part.evaluate((element) => {
          const range = document.createRange()
          range.selectNodeContents(element)
          const bounds = element.getBoundingClientRect()
          return Math.max(0, ...Array.from(range.getClientRects(), (rect) => Math.max(bounds.left - rect.left, rect.right - bounds.right)))
        })
        expect(overflow, `${name} at ${width} px`).toBeLessThanOrEqual(1)
      }
      const status = await banner.locator('[data-part="sentence"]').boundingBox()
      const time = await estimate.boundingBox()
      if (width === 1440) expect(time!.y).toBe(status!.y)
      else expect(time!.y).toBeGreaterThan(status!.y)
    }

    await page.setViewportSize({ width: 420, height: 860 })
    const summaryText = estimate.locator('..')
    const beforeTick = await summaryText.boundingBox()
    await page.clock.setFixedTime(firstFinish - 9_500)
    await expect(estimate).toContainText(said('9s'))
    expect(await summaryText.boundingBox()).toEqual(beforeTick)

    await page.clock.setFixedTime(Math.min(...finishes.map(({ high }) => high)) + 1000)
    await expect(banner).toContainText('No progress for')
    for (const { title, label } of runs.slice(0, 3)) {
      const clipped = await named(title).evaluate((trigger, short) => {
        const label = Array.from(trigger.children).find((element) => element.textContent.replace(/[⁦-⁩]/g, '').trim() === `${short} ·`)
        if (!label) return Infinity
        const range = document.createRange()
        range.selectNodeContents(label)
        const bounds = trigger.getBoundingClientRect()
        return Math.max(0, ...Array.from(range.getClientRects(), (rect) => Math.max(bounds.left - rect.left, rect.right - bounds.right)))
      }, label)
      expect(clipped).toBeLessThanOrEqual(1)
    }
    const list = banner.getByRole('list')
    const overflow = await list.evaluate((element) => element.scrollHeight - element.clientHeight)
    expect(overflow, 'three open cards outgrow the cap on a short window').toBeGreaterThan(0)
    await list.evaluate((element) => element.scrollTo(0, element.scrollHeight))
    await expect(summary).toBeInViewport()
    await expect(rows.nth(2).getByRole('img', { name: /^val_bpb / })).toBeInViewport()
    await expect(composer(page)).toBeInViewport()
    await page.setViewportSize({ width: 1440, height: 900 })
    await page.clock.setFixedTime(Date.now())
  })

  await test.step('the summary folds to one line over every bar, and opens back to its rows', async () => {
    await summary.click()
    await expect(rows).toHaveCount(0)
    await expect(banner.getByRole('progressbar')).toHaveCount(3)
    await expect(summary).toBeFocused()
    await themes(page, banner, info, 'summary-folded')
    await summary.click()
    await expect(rows).toHaveCount(3)
  })

  await test.step('Escape folds the card the focus is in, and it stays folded', async () => {
    for (const index of [0, 1, 2]) await expect(rows.nth(index).getByRole('img', { name: /^val_bpb / })).toBeVisible()
    await rows.nth(1).getByRole('button', { name: 'Show less', exact: true }).focus()
    await page.keyboard.press('Shift+Tab')
    await page.keyboard.press('Shift+Tab')
    await page.keyboard.press('Shift+Tab')
    await expect(named(runs[1].title)).toBeFocused()
    const card = page.getByRole('dialog')
    await expect(card.getByRole('link', { name: runs[1].title })).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(card).toHaveCount(0)
    await expect(named(runs[1].title)).toBeFocused()
    await expect(rows.nth(1).getByRole('img', { name: /^val_bpb / })).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(rows.nth(1).getByRole('button', { name: 'Show more', exact: true })).toBeFocused()
    await expect(rows.nth(1).getByRole('img', { name: /^val_bpb / })).toHaveCount(0)
    await expect(rows.nth(0).getByRole('img', { name: /^val_bpb / })).toBeVisible()
    await themes(page, banner, info, 'summary-row-folded')
    await summary.click()
    await summary.click()
    await expect(rows.nth(1).getByRole('button', { name: 'Show more', exact: true })).toBeVisible()
  })

  await test.step('an unrelated run keeps its whole title and leaves the sweep its labels', async () => {
    agent.script('Also check the tokenizer.', [{ run: ['set -e', ...start(3, 4)].join('\n') }, { say: 'Launched the check.' }])
    await send(page, 'Also check the tokenizer.')
    await expect(rows).toHaveCount(4)
    await expect(banner).toContainText(said('Step 20 of 30'), { timeout: 60_000 })
    await expect(banner).not.toContainText('… at 2e15 FLOPs')
    for (const { title, label } of runs) await expect(named(title)).toContainText(said(label))
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
  writeFileSync(rules, JSON.stringify({ rules: [{ name: 'nan-loss', text: 'Stop when the loss is NaN on 3 reports in a row.', does: 'stop', when: { kind: 'nonFinite', signal: 'loss', reports: 3 } }] }))
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
    await expect(banner).toContainText('Stop when the loss is NaN on 3 reports in a row.')
    await expect(banner).toContainText('nan-loss')
    await expect(banner).toContainText('No model')
    const { statusText }: { statusText: string } = await (await page.request.get(`${orx.url}/api/runs/${runId}/monitor`)).json()
    const stop = statusText.split('\n').find(line => /^stop\s/.test(line))
    // A pass counts every report since the last, so a fast run is stopped a few reports past the third.
    for (const fact of ['Auto-stopped', 'rule', 'nan-loss', 'NaN', 'No model']) {
      expect(stop).toContain(fact)
    }
    expect(stop).toMatch(/\b\d+ reports\b/)
    expect(stop).toMatch(/\bstep 10 to \d+\b/)
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

/**
 * Light and dark stills of `target`, or the whole window, into `$ORX_E2E_SHOTS` for a person to look at; `around`
 * widens a still of `target` to the window's left edge and that many pixels above and below it.
 */
async function keep(page: Page, target: Locator | Page, name: string, around?: number) {
  const dir = process.env.ORX_E2E_SHOTS
  if (!dir) return
  mkdirSync(dir, { recursive: true })
  for (const scheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: scheme })
    await expect(page.locator('html')).toHaveAttribute('data-theme', scheme)
    await expect.poll(() => page.evaluate(() => document.getAnimations().filter((a) => a instanceof CSSTransition).length)).toBe(0)
    const file = path.join(dir, `${name}-${scheme}.png`)
    const box = around === undefined || target === page ? null : await (target as Locator).boundingBox()
    if (box) await page.screenshot({ path: file, clip: { x: 0, y: Math.max(0, box.y - around!), width: box.x + box.width + 8, height: box.height + 2 * around! } })
    else await target.screenshot({ path: file })
  }
  await page.emulateMedia({ colorScheme: 'light' })
}

/** A payload that errs at each of `errors`' steps once its gate opens, and holds at step 30 until `end` opens. */
function erring(orx: Orx, name: string, errors: [string, number, string][]) {
  return payload(orx, name, [...errors.map(([gate]) => gate), 'end'], `
process.stdout.write('::orx:: {"step":0,"of":40,"unit":"steps","t":0}\\n')
const errors = ${JSON.stringify(errors)}
for (let step = 1; step <= 40; step++) {
  await tick()
  for (const [name, at, line] of errors) if (step === at) { gate(name); console.error(line) }
  if (step === 30) gate('end')
  process.stdout.write('::orx:: {"step":' + step + ',"m":{"loss":' + (2 / step).toFixed(4) + '}}\\n')
}`)
}

/** The shell an agent runs to settle the open flag of `run` the way `choice` says. */
const settle = (exp: string, run: string, choice: string, reason: string) =>
  `f=$(orx exp status ${exp} --run ${run} --json | ${process.execPath} -e 'const status = JSON.parse(require("node:fs").readFileSync(0, "utf8")); console.log(status.flags.findLast((flag) => !["kept", "stopped"].includes(flag.state)).id)') && orx exp flag ${run} --flag "$f" --${choice} --reason '${reason}'`

test('an error flags the run and wakes its Agentic agent once, which confirms it and keeps the run going', async ({ page, project, agent, orx }, info) => {
  test.slow()
  await page.request.post(`${orx.url}/api/settings/ui-state`, { headers: { origin: orx.url }, data: { runBanner: 'strip' } })
  const run = erring(orx, `flagged-${project.id}`, [['err', 6, 'RuntimeError: CUDA error: device-side assert triggered']])
  const banner = await launch(page, agent, project, 'Flagged', run.command)
  const shown = (await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json()).find((item: { run: { projectId: string } }) => item.run.projectId === project.id)
  const { id: runId, experimentId } = shown.run
  const woken = `woken-${project.id}`
  agent.script('was flagged for review', [
    { think: 'Checking the evidence in the log.' },
    { hold: woken },
    { run: settle(experimentId, runId, 'keep', 'One bad batch on a warm GPU; the loss kept falling.') },
    { say: 'Looked at the log: one bad batch, and the loss kept falling. Keeping it going.' },
  ])
  const result = `flagged-result-${project.id}`
  agent.script('finished with status', [{ hold: result }, { say: 'Flagged is done.' }])
  await run.open('err')

  await test.step('the wake reaches the agent with the line and how to settle it, while the run keeps going', async () => {
    await agent.reached(woken)
    const wake = readFileSync(orx.agentLog, 'utf8').split('\n').filter((line) => line.includes('was flagged for review')).at(-1)!
    const text = JSON.parse(wake.slice(wake.indexOf('{'))).params.prompt.map((part: { text?: string }) => part.text ?? '').join('')
    expect(text).toContain(`[orx] Run ${runId} (experiment ${experimentId}) was flagged for review.`)
    expect(text).toContain('  | RuntimeError: CUDA error: device-side assert triggered')
    expect(text).toContain('OpenResearch checked this itself, with no model')
    expect(text).toMatch(new RegExp(`orx exp flag ${runId} --flag f[0-9a-f]{8} --keep`))
    await expect(banner).toContainText('is reading the evidence')
    await expect(banner.locator('[data-run-frame]').getByText('A new kind of error in the log:')).toHaveCount(1)
    await expect(page.getByRole('img', { name: 'A run was flagged' })).toBeVisible()
    await keep(page, banner, 'agentic-line-looking')
  })

  await test.step('the agent keeps it going: its turn shows in the chat, and the journal says who settled it', async () => {
    agent.release(woken)
    const reply = page.getByText('Looked at the log: one bad batch, and the loss kept falling. Keeping it going.', { exact: true })
    await expect(reply).toBeVisible()
    await expect(page.getByRole('img', { name: 'A run was flagged' })).toHaveCount(0)
    await keep(page, page, 'agentic-wake-turn')
    await run.open('end')
    await agent.reached(result)
    await expect(banner).toContainText('Done', { timeout: 60_000 })
    await openRunPage(page, banner, /Flagged/)
    const watch = page.locator('section', { has: page.getByRole('heading', { name: 'Watch', exact: true }) })
    const flagged = watch.getByRole('button', { name: /A new kind of error in the log/ })
    await expect(flagged).toHaveAttribute('aria-expanded', 'false')
    await expect(watch.getByText(/E2E Agent chose to keep going/)).toHaveCount(0)
    await flagged.press('Enter')
    await expect(flagged).toHaveAttribute('aria-expanded', 'true')
    await expect.poll(async () => (await watch.innerText()).replace(/[⁦-⁩]/g, ''))
      .toMatch(/A new kind of error in the log: RuntimeError\.\s+E2E Agent chose to keep going\. One bad batch on a warm GPU; the loss kept falling\./)
    await keep(page, watch, 'journal-settled')
    await themes(page, watch, info, 'journal-settled')
    agent.release(result)
  })
})

test('an Independent flag gets one reminder then closes when both reviews leave it open', async ({ page, project, agent, orx }, info) => {
  test.slow()
  await page.request.post(`${orx.url}/api/settings/ui-state`, {
    headers: { origin: orx.url }, data: { preferredAutonomy: 'agentic', runBanner: 'strip' },
  })
  const run = erring(orx, `unsettled-${project.id}`, [['err', 6, 'RuntimeError: One batch failed']])
  const banner = await launch(page, agent, project, 'Training', run.command)
  agent.script('was flagged for review', [
    { hold: 'before-reading' },
    { say: 'Reviewing the log.' },
    { hold: 'reading', onCancel: 'exit' },
  ])
  agent.script('ended without settling flag', [
    { say: 'I checked the run again.' },
    { hold: 'reminder' },
  ])
  agent.script('finished with status', [{ say: 'Training finished.' }])

  async function capture(name: string) {
    for (const width of [1440, 420]) {
      await page.setViewportSize({ width, height: 900 })
      await keep(page, banner, `s9-${name}-${width}`)
    }
    await page.setViewportSize({ width: 1440, height: 900 })
  }

  await run.open('err')
  await agent.reached('before-reading')
  await expect(banner.locator('[data-run-frame]').getByRole('status')).toContainText('Waking')
  await expect(banner.getByRole('button', { name: /^Ask / })).toHaveCount(0)
  await capture('waking')
  agent.release('before-reading')

  await agent.reached('reading')
  await expect(banner.locator('[data-run-frame]').getByRole('status')).toContainText('is reading the evidence')
  await expect(banner.getByRole('button', { name: 'Keep going', exact: true })).toBeVisible()
  await expect(banner.getByRole('button', { name: 'Stop run', exact: true })).toBeVisible()
  await capture('reading')
  await page.getByLabel('Stop', { exact: true }).click()
  await expect(banner.locator('[data-run-frame]').getByRole('status')).toHaveText('Queued')
  await expect(banner.getByRole('button', { name: /^Ask / })).toHaveCount(0)
  await capture('queued')

  agent.script('was flagged for review', [{ say: 'I continued reviewing the log.' }])
  await send(page, 'Review the training log.')
  await expect(page.getByText('I continued reviewing the log.', { exact: true })).toBeVisible()
  await agent.reached('reminder')
  await expect(banner.locator('[data-run-frame]').getByRole('status')).toContainText('is reading the evidence')
  agent.release('reminder')
  await expect(banner.getByRole('button', { name: 'Keep going', exact: true })).toHaveCount(0)
  await expect(page.getByRole('img', { name: 'A run was flagged' })).toHaveCount(0)
  await capture('closed')
  await openRunPage(page, banner, /Training/)
  const watch = page.locator('section', { has: page.getByRole('heading', { name: 'Watch', exact: true }) })
  const flag = watch.getByRole('button', { name: /A new kind of error in the log/ })
  await expect(flag).toHaveAttribute('aria-expanded', 'false')
  await flag.press('Enter')
  await expect(flag).toHaveAttribute('aria-expanded', 'true')
  await expect(watch).toContainText('left it unsettled twice.')
  await run.open('end')
  await expect(page.getByText('Training finished.', { exact: true })).toBeVisible()
})

test('a Copilot flag opens the strip with its evidence and waits: Keep going settles it, Ask sends it to the agent', async ({ page, project, agent, orx }, info) => {
  test.slow()
  const autonomy = (value: string) => page.request.post(`${orx.url}/api/settings/ui-state`, { headers: { origin: orx.url }, data: { preferredAutonomy: value, runBanner: 'strip' } })
  await autonomy('copilot')
  try {
    const run = erring(orx, `copilot-${project.id}`, [
      ['err', 6, 'RuntimeError: CUDA error: device-side assert triggered'],
      ['shard', 14, 'ValueError: shard 3 of the training set is corrupt (bad header)'],
    ])
    const banner = await launch(page, agent, project, 'Copiloted', run.command)
    const shown = (await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json()).find((item: { run: { projectId: string } }) => item.run.projectId === project.id)
    const { id: runId, experimentId } = shown.run
    await test.step('an interrupted earlier result leaves the chat idle before the next flag', async () => {
      const prior = payload(orx, `prior-${project.id}`, ['finish'], `
process.stdout.write('::orx:: {"step":0,"of":1}\\n')
gate('finish')`)
      const interrupted = `interrupted-result-${project.id}`
      agent.script('Launch a prior result.', [
        { run: [
          'set -e',
          `id=$(orx create-experiment ${project.id} --title PriorResult --run-command '${prior.command}' | sed -n 's/^ *id: *//p')`,
          'orx exp run "$id" --backend local',
          'orx exp wake "$id"',
        ].join('\n') },
        { say: 'Prior result launched.' },
      ])
      agent.script('finished with status', [{ hold: interrupted, onCancel: 'exit' }])
      await send(page, 'Launch a prior result.')
      await expect(page.getByText('Prior result launched.', { exact: true })).toBeVisible({ timeout: 60_000 })
      await prior.open('finish')
      await agent.reached(interrupted)
      await page.getByLabel('Stop', { exact: true }).click()
      await expect(page.getByLabel('Stop', { exact: true })).toBeHidden()
    })
    await run.open('err')

    await test.step('the strip opens by itself with the evidence and three choices; the sidebar row turns amber', async () => {
      await expect(banner.locator('pre')).toHaveText('RuntimeError: CUDA error: device-side assert triggered')
      await expect(banner.locator('[data-run-frame]').getByText('A new kind of error in the log:')).toHaveCount(1)
      for (const name of [/^Stop run$/, /^Keep going$/, /^Ask /]) await expect(banner.getByRole('button', { name })).toBeVisible()
      await expect(banner).toContainText('The run keeps going until you choose.')
      await expect(page.getByRole('img', { name: 'A run needs your attention' })).toBeVisible()
      await themes(page, banner, info, 'strip-copilot-flag')
      for (const width of [1440, 420]) {
        await page.setViewportSize({ width, height: 900 })
        await keep(page, banner, `s9-copilot-${width}`)
      }
      await page.setViewportSize({ width: 1440, height: 900 })
      await keep(page, page.locator('.session-row', { has: page.getByRole('img', { name: 'A run needs your attention' }) }), 'sidebar-amber', 90)
    })

    await test.step('Keep going settles the flag for good and the run goes on', async () => {
      await banner.getByRole('button', { name: 'Keep going', exact: true }).click()
      await expect(banner.getByRole('button', { name: 'Keep going', exact: true })).toHaveCount(0)
      await expect(page.getByRole('img', { name: 'A run needs your attention' })).toHaveCount(0)
      const { flags: [flag] } = await (await page.request.get(`${orx.url}/api/runs/${runId}/monitor`)).json()
      expect([flag.state, flag.choice.by, flag.choice.c]).toEqual(['kept', 'user', 'keep'])
    })

    await test.step('Ask can queue behind a turn and can wake the agent again after a lost reply and handback', async () => {
      const busy = `busy-${project.id}`
      const waking = `waking-${project.id}`
      const asked = `asked-${project.id}`
      const stopped = `stopped-${project.id}`
      agent.script('Review the experiment setup.', [{ hold: busy }, { say: 'I reviewed the experiment setup.' }])
      agent.script('The user asked you to look at a flag', [
        { run: settle(experimentId, runId, 'ask', 'This choice needs the user.') },
        { say: 'The flag is back with you.' },
      ])
      agent.script('The user asked you to look at a flag', [
        { hold: waking },
        { think: 'Checking the shard error in the log.' },
        { hold: asked },
        { run: settle(experimentId, runId, 'stop', 'Shard 3 is corrupt; the rest of this run would train on garbage.') },
        { say: 'Shard 3 is corrupt, so I stopped the run. Shall I rebuild the shard and relaunch?' },
      ])
      agent.script('finished with status', [{ hold: stopped }, { say: 'Copiloted stopped.' }])
      await run.open('shard')
      await expect(banner.locator('pre')).toHaveText('ValueError: shard 3 of the training set is corrupt (bad header)')
      await send(page, 'Review the experiment setup.')
      await agent.reached(busy)
      await page.route(`**/api/runs/${runId}/flag`, async (route) => {
        const saved = await route.fetch()
        expect(saved.ok()).toBe(true)
        await route.abort()
      }, { times: 1 })
      await banner.getByRole('button', { name: /^Ask / }).click()
      await expect(banner.getByText('Queued', { exact: true })).toBeVisible()
      agent.release(busy)
      await expect(page.getByText('The flag is back with you.', { exact: true })).toBeVisible()
      await expect(banner.getByRole('button', { name: /^Ask / })).toBeEnabled()
      await banner.getByRole('button', { name: /^Ask / }).click()
      await agent.reached(waking)
      await expect(banner.getByRole('button', { name: /^Ask / })).toHaveCount(0)
      const row = banner.getByRole('group', { name: 'Copiloted', exact: true })
      await expect(row.getByRole('button', { name: 'Show less', exact: true })).toBeVisible()
      await expect(row).toContainText('Waking')
      agent.release(waking)
      await agent.reached(asked)
      await expect(row).toContainText('is reading the evidence')
      await themes(page, banner, info, 'strip-copilot-asked')
      await keep(page, banner, 'copilot-asked')
      agent.release(asked)
      await expect(page.getByText('Shard 3 is corrupt, so I stopped the run. Shall I rebuild the shard and relaunch?', { exact: true })).toBeVisible()
      await agent.reached(stopped)
      await expect(banner).toContainText('Auto-stopped')
      await expect(page.getByRole('img', { name: 'A run needs your attention' })).toHaveCount(0)
      const { flags } = await (await page.request.get(`${orx.url}/api/runs/${runId}/monitor`)).json()
      expect(flags.map((flag: { state: string; choice: { by: string } }) => [flag.state, flag.choice.by])).toEqual([['kept', 'user'], ['stopped', 'agent']])
      agent.release(stopped)
      await expect(page.getByText('Copiloted stopped.', { exact: true })).toBeVisible()
      await keep(page, page, 'copilot-answered')
    })
  } finally {
    await autonomy('agentic')
  }
})

test("a chat's hover card says how each experiment is going and how the finished ones ended", async ({ page, project, agent, orx }, info) => {
  test.slow()
  const autonomy = (value: string) => page.request.post(`${orx.url}/api/settings/ui-state`, { headers: { origin: orx.url }, data: { preferredAutonomy: value } })
  const stop = path.join(orx.root, `hover-stop-${project.id}`)
  const ticking = (name: string, body: string) => payload(orx, `${name}-${project.id}`, [], `import { existsSync } from 'node:fs'\n${body}`).command
  const quick = ticking('baseline', `
process.stdout.write('::orx:: {"step":0,"of":8,"unit":"steps","t":0}\\n')
for (let step = 1; step <= 8; step++) { await tick(); process.stdout.write('::orx:: {"step":' + step + ',"t":' + step + ',"m":{"val_loss":' + (0.5 / step).toFixed(4) + '}}\\n') }`)
  const broken = ticking('broken', `
process.stdout.write('::orx:: {"step":0,"of":100,"unit":"steps","t":0}\\n')
for (let step = 1; step <= 4; step++) { await tick(); process.stdout.write('::orx:: {"step":' + step + ',"t":' + step + '}\\n') }
process.exit(3)`)
  // Live runs tick until the test writes the stop file.
  const live = (name: string, err: string | null) => ticking(name, `
const started = performance.now()
process.stdout.write('::orx:: {"plan":[{"phase":"prepare","of":3,"unit":"steps"},{"phase":"train","of":5000,"unit":"steps"}]}\\n')
process.stdout.write('::orx:: {"phase":"prepare","step":0,"of":3,"unit":"steps","t":0}\\n')
for (let step = 1; step <= 3; step++) { await tick(); process.stdout.write('::orx:: {"step":' + step + '}\\n') }
process.stdout.write('::orx:: {"phase":"train","step":0,"of":5000,"unit":"steps","t":0}\\n')
for (let step = 1; step <= 5000 && !existsSync(${JSON.stringify(stop)}); step++) {
  await tick()
  if (step === 6 && ${JSON.stringify(err)}) console.error(${JSON.stringify(err)})
  process.stdout.write('::orx:: {"step":' + step + ',"t":' + ((performance.now() - started) / 1000) + ',"m":{"val_loss":' + (1 / Math.sqrt(step)).toFixed(4) + '}}\\n')
}`)
  const create = (title: string, command: string) => `orx create-experiment ${project.id} --title '${title}' --run-command '${command}' | sed -n 's/^ *id: *//p'`
  const launch = (title: string, command: string) => `orx exp run "$(${create(title, command)})" --backend local`
  type Row = { experiment: { title: string }; currentRun: { id: string; status: string; monitor?: { flag?: { state: string } | null } | null } | null }
  /** Until some chat lists `count` experiments that `ready` accepts; ended runs stay shown until their notice is answered. */
  const settled = (count: number, ready: (rows: Row[]) => boolean) => expect.poll(async () => {
    const shown: { sessionId: string }[] = await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json()
    for (const id of new Set(shown.map((item) => item.sessionId))) {
      const { experiments } = await (await page.request.get(`${orx.url}/api/chat/sessions/${id}/experiments`)).json()
      if (experiments.length === count && ready(experiments)) return true
    }
    return false
  }, { timeout: 60_000 }).toBe(true)
  /** A turn that runs `commands` and holds until its runs reach `count` and `ready`, so their results come back as one turn. */
  const turn = async (prompt: string, commands: string[], count: number, ready: (rows: Row[]) => boolean) => {
    const hold = `${prompt}-${project.id}`
    agent.script(prompt, [{ run: ['set -e', ...commands].join('\n') }, { hold }, { say: `${prompt} Done.` }])
    agent.script('finished with status', [{ say: `Read what came of: ${prompt}` }])
    await send(page, prompt)
    await agent.reached(hold)
    await settled(count, ready)
    agent.release(hold)
    await expect(page.getByText(`Read what came of: ${prompt}`, { exact: true })).toBeVisible({ timeout: 60_000 })
  }
  const card = page.locator('[data-session-hover-card]')
  const chat = (prompt: string) => page.locator('[data-sidebar-keyboard-row]').filter({ hasText: prompt })
  const hover = async (prompt: string) => {
    await page.mouse.move(0, 0)
    await expect(card).toHaveCount(0)
    await chat(prompt).hover()
    await expect(card).toBeVisible()
  }
  const shots = async (prompt: string, name: string) => {
    for (const width of [1440, 420]) {
      await page.setViewportSize({ width, height: 900 })
      if (width === 420) await page.getByRole('button', { name: 'Show sidebar' }).click()
      await hover(prompt)
      await keep(page, page, `${name}-${width}`)
    }
    await page.setViewportSize({ width: 1440, height: 900 })
    await hover(prompt)
    await themes(page, card, info, name)
  }
  const status = (rows: Row[], title: string) => rows.find((row) => row.experiment.title === title)?.currentRun

  await autonomy('copilot')
  try {
    await project.open(page)
    await turn('Try four ideas.', [
      launch('Baseline', quick),
      launch('Smaller batch', broken),
      launch('Wider layers', live('wider', null)),
      launch('Higher learning rate', live('higher', 'RuntimeError: CUDA error: device-side assert triggered')),
      create('Dropout ablation', 'true'),
    ], 5, (rows) => status(rows, 'Baseline')?.status === 'done' && status(rows, 'Smaller batch')?.status === 'failed'
      && status(rows, 'Wider layers')?.status === 'running' && status(rows, 'Higher learning rate')?.monitor?.flag?.state === 'waiting')

    await test.step('the card leads with the flag waiting on the user, then the live run, then how each finished one ended', async () => {
      await shots('Try four ideas', 'hover-mixed')
      const rows = card.locator('[data-experiment-row]')
      await expect(rows).toHaveCount(5)
      await expect(rows.nth(0)).toContainText('Higher learning rate')
      await expect(rows.nth(0)).toContainText('Waiting for your choice')
      await expect(rows.nth(0).getByRole('img', { name: 'Flagged' })).toBeVisible()
      await expect(rows.nth(1)).toContainText('Wider layers')
      await expect(rows.nth(1)).toContainText(said('of 5,000'))
      await expect(rows.nth(1)).toContainText('val_loss')
      const row = (title: string) => card.locator('[data-experiment-row]', { hasText: title })
      await expect(row('Baseline')).toContainText('Done in')
      await expect(row('Baseline')).toContainText('val_loss')
      await expect(row('Smaller batch')).toContainText('Failed')
      await expect(row('Dropout ablation')).toContainText('Not run yet')
    })

    await test.step('focus moves between rows and Escape goes back to the chat', async () => {
      await page.mouse.move(0, 0)
      await expect(card).toHaveCount(0)
      await chat('Try four ideas').focus()
      await expect(card).toBeVisible()
      await page.keyboard.press('Tab')
      await expect(card).toBeFocused()
      await page.keyboard.press('Tab')
      await expect(card.locator('[data-experiment-row]').nth(0)).toBeFocused()
      await page.keyboard.press('ArrowDown')
      await expect(card.locator('[data-experiment-row]').nth(1)).toBeFocused()
      await page.keyboard.press('Escape')
      await expect(chat('Try four ideas')).toBeFocused()
      await expect(card).toHaveCount(0)
    })

    await test.step('a chat whose experiments all finished reads each result', async () => {
      await page.keyboard.press('ControlOrMeta+n')
      await expect(page.getByRole('feed', { name: 'Messages' })).toHaveCount(0)
      await turn('Try two baselines.', ['Linear baseline', 'Learning rate 0.10'].map((title) => launch(title, quick)), 2,
        (rows) => rows.every((row) => row.currentRun?.status === 'done'))
      await shots('Try two baselines', 'hover-finished')
      await expect(card.locator('[data-experiment-row]')).toHaveCount(2)
    })

    await test.step('a long result wraps inside the card and keeps its value and its comparison', async () => {
      const long = ticking('long', `
const scale = Number(process.argv[2])
process.stdout.write('::orx:: {"step":0,"of":4,"unit":"steps","t":0}\\n')
for (let step = 1; step <= 4; step++) { await tick(); process.stdout.write('::orx:: {"step":' + step + ',"t":' + step + ',"m":{"bpb":' + (scale / step).toFixed(4) + '}}\\n') }`)
      const parent = path.join(orx.root, `hover-parent-${project.id}`)
      const view = path.join(orx.root, `hover-view-${project.id}.xml`)
      writeFileSync(view, '<signal name="bpb" label="Validation bits per byte on held-out shards" digits="3" better="lower"/><view headline="bpb"><chart y="bpb"/></view>')
      await page.keyboard.press('ControlOrMeta+n')
      await expect(page.getByRole('feed', { name: 'Messages' })).toHaveCount(0)
      await turn('Run the long baseline.', [`id=$(${create('Long baseline', `${long} 0.9`)})`, `echo "$id" > ${parent}`, `orx exp view set "$id" --file ${view}`, 'orx exp run "$id" --backend local'], 1,
        (rows) => rows.every((row) => row.currentRun?.status === 'done'))
      await turn('Try a smaller learning rate on it.', [
        `id=$(orx create-experiment ${project.id} --parent "$(cat ${parent})" --title 'Smaller learning rate' --run-command '${long} 0.7' | sed -n 's/^ *id: *//p')`,
        `orx exp view set "$id" --file ${view}`,
        'orx exp run "$id" --backend local',
      ], 2, (rows) => rows.every((row) => row.currentRun?.status === 'done'))
      for (const width of [1440, 420]) {
        await page.setViewportSize({ width, height: 900 })
        if (width === 420) await page.getByRole('button', { name: 'Show sidebar' }).click()
        await hover('Run the long baseline')
        const row = card.locator('[data-experiment-row]', { hasText: 'Smaller learning rate' })
        await expect(row).toContainText('Validation bits per byte on held-out shards')
        await expect(row).toContainText(said('0.175'))
        await expect(row).toContainText('better than parent')
        const scroller = card.locator('ul').locator('..')
        expect(await scroller.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true)
        expect(await row.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true)
        await keep(page, card, `hover-long-${width}`)
      }
      await page.setViewportSize({ width: 1440, height: 900 })
    })

    await test.step('a chat with many experiments scrolls them under its header', async () => {
      await page.keyboard.press('ControlOrMeta+n')
      await expect(page.getByRole('feed', { name: 'Messages' })).toHaveCount(0)
      await turn('Try twelve seeds.', Array.from({ length: 12 }, (_, index) => launch(`Seed ${index + 1}`, quick)), 12,
        (rows) => rows.every((row) => row.currentRun?.status === 'done'))
      await shots('Try twelve seeds', 'hover-many')
      const scroller = card.locator('ul').locator('..')
      expect(await scroller.evaluate((element) => element.scrollHeight > element.clientHeight)).toBe(true)
    })

    await test.step('the live runs end and their results come back', async () => {
      await chat('Try four ideas').click()
      await turn('Stop the rest.', [`touch ${stop}`], 5, (rows) => rows.every((row) => !row.currentRun || ['done', 'failed'].includes(row.currentRun.status)))
    })
  } finally {
    writeFileSync(stop, '')
    await autonomy('agentic')
  }
})
